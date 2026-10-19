//! The HTML tokenizer, ported from Lexbor's `html/tokenizer.c` and
//! `html/tokenizer/state*.c`.
//!
//! The port keeps Lexbor's shape: a state is a function that consumes as much
//! of `[data, end)` as it can and returns where it stopped, the driver calls the
//! current state until the input is used up, and end of file is one more pass
//! over a single NUL byte with `is_eof` set. Each Rust state function names the
//! Lexbor function it ports.
//!
//! What changed, and only for ownership and safety (no behaviour changes):
//!
//! - **Positions are indices**, not pointers. The input is one slice, and
//!   Lexbor's separate one-byte end-of-file buffer is the virtual index
//!   `input.len()`, which reads as NUL. Lexbor's NULL pointers are `None`.
//! - **The temporary buffer is a `Vec<u8>`**, whose length is Lexbor's `pos`.
//! - **Every allocation is fallible** (`falloc`): a failure sets the status and
//!   stops, as Lexbor's does.
//! - **The token-done callback is a [`TokenSink`]**, handed the tokenizer
//!   itself, as Lexbor's tree builder is handed `tkz`.
//! - **The names table is the sink's** ([`TokenSink::names`]), not a second
//!   reference the tokenizer holds: the tree builder writes the document that
//!   owns the table, and Lexbor's tokenizer and document share it the same way.
//!
//! Lexbor's chunked input is kept as an API (`chunk` may be called more than
//! once) because its states are written for it, but Makiri passes one chunk.

#![forbid(unsafe_code)]

mod error;
mod state;
mod state_comment;
mod state_doctype;
mod state_text;
#[cfg(test)]
pub(crate) mod tests;

pub use error::ErrorId;
pub use state::{cdata_section_before, data_before, plaintext_before, State};
pub use state_text::{rawtext_before, rcdata_before, script_data_before};

use crate::falloc::{Reserve, VecPush};
use crate::html::names::Names;
use crate::html::res::names as res;

/// `lxb_html_token_type`: flags on a token.
pub mod flags {
    pub const CLOSE: u8 = 0x01;
    pub const CLOSE_SELF: u8 = 0x02;
    pub const FORCE_QUIRKS: u8 = 0x04;
    pub const DONE: u8 = 0x08;
}

/// `lxb_html_token_attr_type_t`: flags on a token attribute.
pub mod attr_flags {
    pub const NAME_NULL: u8 = 0x01;
    pub const VALUE_NULL: u8 = 0x02;
}

/// `lxb_html_token_attr_t`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenAttr {
    pub name_begin: Option<usize>,
    pub name_end: Option<usize>,
    pub value_begin: Option<usize>,
    pub value_end: Option<usize>,
    /// The interned local-name id (`attr->name`).
    pub name: Option<usize>,
    /// `[start, end)` in the tokenizer's attribute-value store, once set
    /// (`attr->value`, `value_size`).
    pub value: Option<(usize, usize)>,
    pub flags: u8,
}

/// `lxb_html_token_t`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Token {
    pub begin: Option<usize>,
    pub end: Option<usize>,
    /// The token's text is the tokenizer buffer's first `text` bytes
    /// (`text_start` is always the buffer start); `None` before it is set.
    pub text: Option<usize>,
    pub attrs: Vec<TokenAttr>,
    /// For text, the number of U+0000 bytes in it; for a processing
    /// instruction, the target's length (see Lexbor's `lxb_html_token_t`).
    pub null_count: usize,
    pub tag_id: usize,
    pub flags: u8,
}

impl Token {
    /// `lxb_html_token_clean`, keeping the attribute list's allocation.
    fn clean(&mut self) {
        let mut attrs = core::mem::take(&mut self.attrs);
        attrs.clear();
        *self = Token {
            attrs,
            ..Token::default()
        };
    }
}

/// Why tokenizing stopped (`lxb_status_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// `LXB_STATUS_ERROR_MEMORY_ALLOCATION`.
    Oom,
    /// `LXB_STATUS_ERROR_OVERFLOW`.
    Overflow,
    /// `LXB_STATUS_ERROR`: the sink stopped the tokenizer.
    Error,
}

/// A recorded parse error (`lxb_html_tokenizer_error_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub pos: Option<usize>,
    pub id: ErrorId,
}

/// A sink's request to stop tokenizing (Lexbor's NULL return from the
/// callback), with why: the tokenizer's status reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The sink could not allocate.
    Oom,
    /// Anything else.
    Error,
}

/// What a tokenizer is set up with, before its first chunk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `LXB_HTML_TOKENIZER_OPT_ATTR_KEEP_DUPLICATE`: hand a start tag's
    /// repeated attributes on rather than dropping them. Lexbor's parser
    /// sets it, and its tree builder drops a repeat by name and namespace.
    pub keep_duplicate: bool,
    /// `NS__UNDEF` as the current namespace is a foreign namespace, not "no
    /// tree": a fragment whose context's namespace the parser was given as
    /// none.
    pub undef_is_foreign: bool,
}

/// The text-content states a tree builder switches the tokenizer into after
/// a start tag (the Standard's "switch the tokenizer to the ... state"),
/// which only an appropriate end tag leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextContent {
    Rcdata,
    Rawtext,
    ScriptData,
}

/// The receiver of each finished token: Lexbor's `callback_token_done`.
///
/// It is handed the tokenizer itself, as Lexbor's tree builder is handed `tkz`,
/// so it can read the token's text and switch the tokenizer's state.
pub trait TokenSink {
    fn token(&mut self, tkz: &mut Tokenizer<'_, '_>) -> Result<(), Stop>;

    /// The table tag and attribute names are interned into (Lexbor's
    /// `tkz->tags` and `tkz->attrs`, which are the document's).
    fn names(&mut self) -> &mut Names;
}

/// `lxb_html_tokenizer_t`.
pub struct Tokenizer<'i, 's> {
    pub(crate) input: &'i [u8],
    pub(crate) state: State,
    pub(crate) state_return: Option<State>,
    pub(crate) sink: Option<&'s mut dyn TokenSink>,
    pub token: Token,
    /// Lexbor's temporary buffer: `start` is index 0, `pos` is the length.
    pub(crate) buf: Vec<u8>,
    /// Attribute values of the current token (Lexbor's `attrs_mraw` copies).
    pub(crate) values: Vec<u8>,
    pub(crate) errors: Vec<ParseError>,

    /// `markup`: the rest of a keyword being matched across a chunk end.
    pub(crate) markup: &'static [u8],
    /// `markup` as a position, for a numeric reference's errors.
    pub(crate) markup_pos: Option<usize>,
    /// `temp`: where an RCDATA/RAWTEXT/script end tag began.
    pub(crate) temp: Option<usize>,
    /// The tag of the last start tag, which decides whether an end tag in
    /// RCDATA/RAWTEXT/script data is "appropriate".
    tmp_tag_id: usize,

    /// `begin`: where the input not yet appended to the buffer starts.
    pub(crate) begin: usize,
    /// `last`: the end of the last chunk.
    pub(crate) last: usize,

    pub(crate) entity: usize,
    pub(crate) entity_match: Option<usize>,
    pub(crate) entity_start: usize,
    pub(crate) entity_end: usize,
    pub(crate) entity_number: u32,
    pub(crate) is_attribute: bool,

    status: Status,
    pub(crate) is_eof: bool,
    /// The namespace of the tree builder's adjusted current node, which
    /// decides whether `<![CDATA[` opens a CDATA section
    /// (`lxb_html_tokenizer_current_namespace`); `NS__UNDEF` with no tree.
    current_ns: usize,
    options: Options,
}

impl<'i, 's> Tokenizer<'i, 's> {
    /// `lxb_html_tokenizer_create` + `_init` + `_begin`, over `input`, in the
    /// data state, interning names into the sink's table.
    pub fn new(input: &'i [u8], sink: &'s mut dyn TokenSink) -> Self {
        Self::with_options(input, sink, Options::default())
    }

    /// [`new`](Self::new), set up with `options`.
    pub fn with_options(input: &'i [u8], sink: &'s mut dyn TokenSink, options: Options) -> Self {
        Tokenizer {
            input,
            state: data_before,
            state_return: None,
            sink: Some(sink),
            token: Token::default(),
            buf: Vec::new(),
            values: Vec::new(),
            errors: Vec::new(),
            markup: b"",
            markup_pos: None,
            temp: None,
            tmp_tag_id: res::TAG__UNDEF,
            begin: 0,
            last: 0,
            entity: 0,
            entity_match: None,
            entity_start: 0,
            entity_end: 0,
            entity_number: 0,
            is_attribute: false,
            status: Status::Ok,
            is_eof: false,
            current_ns: res::NS__UNDEF,
            options,
        }
    }

    /// `lxb_html_tokenizer_state_set`.
    fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Switch to the `content` state after the start tag `tag_id`, which an
    /// end tag must name to close it (`lxb_html_tokenizer_tmp_tag_id_set`
    /// and `lxb_html_tokenizer_state_set` together, which every caller
    /// paired).
    pub fn enter_text_content(&mut self, content: TextContent, tag_id: usize) {
        self.tmp_tag_id = tag_id;
        self.set_state(match content {
            TextContent::Rcdata => rcdata_before,
            TextContent::Rawtext => rawtext_before,
            TextContent::ScriptData => script_data_before,
        });
    }

    /// Switch to the PLAINTEXT state, which nothing leaves.
    pub fn enter_plaintext(&mut self) {
        self.set_state(plaintext_before);
    }

    /// The fragment parsing algorithm's step: the state its context element
    /// `tag_id` in `ns` puts the tokenizer in, with no end tag appropriate -
    /// no start tag has been seen (`lxb_html_parse_fragment_chunk_begin`).
    pub fn enter_fragment_context(&mut self, scripting: bool, tag_id: usize, ns: usize) {
        self.set_state_by_tag(scripting, tag_id, ns);
        self.tmp_tag_id = res::TAG__UNDEF;
    }

    /// The namespace of the tree builder's adjusted current node, which the
    /// tree builder reports after each token.
    #[inline]
    pub fn set_current_namespace(&mut self, ns: usize) {
        self.current_ns = ns;
    }

    /// Whether the adjusted current node is foreign content, for
    /// `<![CDATA[`.
    #[inline]
    pub(crate) fn in_foreign_content(&self) -> bool {
        let ns = self.current_ns;
        ns != res::NS_HTML && (ns != res::NS__UNDEF || self.options.undef_is_foreign)
    }

    /// The parse errors recorded so far.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// The current token's text (inside the sink).
    pub fn text(&self) -> &[u8] {
        match self.token.text {
            Some(len) => self.buf.get(..len).unwrap_or_default(),
            None => &[],
        }
    }

    /// An attribute's value (inside the sink); `None` when it has none.
    pub fn attr_value(&self, a: &TokenAttr) -> Option<&[u8]> {
        a.value
            .map(|(s, e)| self.values.get(s..e).unwrap_or_default())
    }

    /// `lxb_html_tokenizer_chunk`: tokenize the whole of the input, which is
    /// one chunk.
    pub fn chunk(&mut self) -> Status {
        let end = self.input.len();
        self.is_eof = false;
        self.status = Status::Ok;
        self.last = end;
        let mut data = 0;
        while data < end {
            data = (self.state)(self, data, end);
        }
        self.status
    }

    /// `lxb_html_tokenizer_end`: the end-of-file pass, then the end-of-file
    /// token.
    pub fn end(&mut self) -> Status {
        self.status = Status::Ok;
        /* Lexbor's fake EOF buffer: one NUL at the virtual index `len`. */
        let data = self.input.len();
        let end = data + 1;
        self.is_eof = true;
        while (self.state)(self, data, end) < end {}
        self.is_eof = false;
        if self.status != Status::Ok {
            return self.status;
        }
        self.token.clean();
        self.token.tag_id = res::TAG__END_OF_FILE;
        let _ = self.emit(); /* the status says whether it stopped */
        self.status
    }

    /* ---- reading the input ---- */

    /// The byte at `i`, NUL at and past the end (the end-of-file buffer).
    #[inline]
    pub(crate) fn at(&self, i: usize) -> u8 {
        self.input.get(i).copied().unwrap_or(0)
    }

    /// The input in `[from, to)`, empty where it reaches past the end.
    #[inline]
    fn input_range(&self, from: usize, to: usize) -> &'i [u8] {
        let n = self.input.len();
        self.input.get(from.min(n)..to.min(n)).unwrap_or_default()
    }

    /* ---- the temporary buffer (tokenizer.h's inline helpers) ---- */

    /// `lxb_html_tokenizer_temp_append_data`: append `[begin, data)`.
    /// `Err` with the status set on allocation failure.
    #[inline]
    pub(crate) fn append_data(&mut self, data: usize) -> Result<(), ()> {
        let bytes = self.input_range(self.begin, data);
        self.append(bytes)
    }

    /// `lxb_html_tokenizer_temp_append`.
    #[inline]
    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.buf.falloc_reserve(bytes.len()).is_err() {
            self.status = Status::Oom;
            return Err(());
        }
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    /// `lxb_html_tokenizer_state_append_replace_m`: U+FFFD.
    #[inline]
    pub(crate) fn append_replace(&mut self) -> Result<(), ()> {
        self.append("\u{FFFD}".as_bytes())
    }

    /// `tkz->pos[-1] = 0x0A`, after a CR was appended.
    #[inline]
    pub(crate) fn last_to_lf(&mut self) {
        if let Some(b) = self.buf.last_mut() {
            *b = b'\n';
        }
    }

    /* ---- the token (state.h's macros) ---- */

    /// `lxb_html_tokenizer_state_token_set_begin`.
    #[inline]
    pub(crate) fn token_set_begin(&mut self, at: usize) {
        self.buf.clear();
        self.token.begin = Some(at);
    }

    /// `lxb_html_tokenizer_state_set_text`.
    #[inline]
    pub(crate) fn set_text(&mut self) {
        self.token.text = Some(self.buf.len());
    }

    /// `lxb_html_tokenizer_state_set_tag_m` over the buffer's `[from, len)`.
    pub(crate) fn set_tag_from_buf(&mut self, from: usize) -> Result<(), ()> {
        let name = self.buf.get(from..).unwrap_or_default();
        match intern(&mut self.sink, |names| names.tag_append_lower(name)) {
            Ok(id) => {
                self.token.tag_id = id;
                Ok(())
            }
            Err(_) => {
                self.status = Status::Oom;
                Err(())
            }
        }
    }

    /// `lxb_html_tokenizer_state_set_name_m`: the last attribute's name is
    /// the buffer.
    pub(crate) fn set_name(&mut self) -> Result<(), ()> {
        match intern(&mut self.sink, |names| {
            names.attr_local_name_append(&self.buf)
        }) {
            Ok(id) => {
                if let Some(a) = self.token.attrs.last_mut() {
                    a.name = Some(id);
                }
                Ok(())
            }
            Err(_) => {
                self.status = Status::Oom;
                Err(())
            }
        }
    }

    /// `lxb_html_tokenizer_state_set_value_m`: the last attribute's value is a
    /// copy of the buffer.
    pub(crate) fn set_value(&mut self) -> Result<(), ()> {
        let start = self.values.len();
        if self.values.falloc_reserve(self.buf.len()).is_err() {
            self.status = Status::Oom;
            return Err(());
        }
        self.values.extend_from_slice(&self.buf);
        let end = self.values.len();
        if let Some(a) = self.token.attrs.last_mut() {
            a.value = Some((start, end));
        }
        Ok(())
    }

    /// `lxb_html_tokenizer_state_token_attr_add_m`.
    pub(crate) fn attr_add(&mut self) -> Result<(), ()> {
        if !self.options.keep_duplicate {
            self.attr_last_duplicate();
        }
        if self.token.attrs.falloc_push(TokenAttr::default()).is_err() {
            self.status = Status::Oom;
            return Err(());
        }
        Ok(())
    }

    /// The last attribute, which the states only reach once one was added.
    #[inline]
    pub(crate) fn attr_last(&mut self) -> &mut TokenAttr {
        if self.token.attrs.is_empty() {
            /* Unreachable by the state machine; an empty attribute keeps the
             * port total rather than panicking. */
            let _ = self.token.attrs.falloc_push(TokenAttr::default());
        }
        let n = self.token.attrs.len();
        &mut self.token.attrs[n - 1]
    }

    /// The last attribute's `value_begin`, if there is a last attribute.
    #[inline]
    pub(crate) fn attr_last_value_begin(&self) -> Option<usize> {
        self.token.attrs.last().and_then(|a| a.value_begin)
    }

    /// `lxb_html_tokenizer_state_token_attr_set_name_begin`.
    #[inline]
    pub(crate) fn attr_set_name_begin(&mut self, at: usize) {
        self.buf.clear();
        self.attr_last().name_begin = Some(at);
    }

    /// `lxb_html_tokenizer_state_token_attr_set_value_begin`.
    #[inline]
    pub(crate) fn attr_set_value_begin(&mut self, at: Option<usize>) {
        self.buf.clear();
        self.attr_last().value_begin = at;
    }

    /// `lxb_html_tokenizer_attr_last_duplicate`: drop the last attribute when
    /// an earlier one has the same name.
    fn attr_last_duplicate(&mut self) {
        let Some(last) = self.token.attrs.last() else {
            return;
        };
        let (name, at) = (last.name, last.name_begin);
        let n = self.token.attrs.len();
        if self.token.attrs[..n - 1].iter().any(|a| a.name == name) {
            self.error_at(at, ErrorId::Duat);
            self.token.attrs.pop();
        }
    }

    /// `lxb_html_tokenizer_validate_close_tag`.
    fn validate_close_tag(&mut self) {
        if let Some(a) = self.token.attrs.last() {
            let at = a.name_begin;
            self.error_at(at, ErrorId::Entawiat);
        }
        if self.token.flags & flags::CLOSE_SELF != 0 {
            let at = self.token.end;
            self.error_at(at, ErrorId::Entawitrso);
        }
    }

    /// `lxb_html_tokenizer_error_add` (a failed push is ignored, as there).
    #[inline]
    pub(crate) fn error(&mut self, pos: usize, id: ErrorId) {
        self.error_at(Some(pos), id);
    }

    #[inline]
    pub(crate) fn error_at(&mut self, pos: Option<usize>, id: ErrorId) {
        let _ = self.errors.falloc_push(ParseError { pos, id });
    }

    /// Hand the token to the sink. `Err` when the sink stopped (status set).
    fn emit(&mut self) -> Result<(), ()> {
        let Some(sink) = self.sink.take() else {
            self.status = Status::Error;
            return Err(());
        };
        let r = sink.token(self);
        self.sink = Some(sink);
        if let Err(stop) = r {
            if self.status == Status::Ok {
                self.status = match stop {
                    Stop::Oom => Status::Oom,
                    Stop::Error => Status::Error,
                };
            }
            return Err(());
        }
        Ok(())
    }

    /// `_lxb_html_tokenizer_state_token_done_m`: the duplicate check, the
    /// end-tag check, the callback. `Err` when the callback stopped.
    fn token_done_inner(&mut self) -> Result<(), ()> {
        if !self.options.keep_duplicate {
            self.attr_last_duplicate();
        }
        if self.token.flags & flags::CLOSE != 0 {
            self.validate_close_tag();
        }
        self.emit()
    }

    /// `lxb_html_tokenizer_state_token_done_m`.
    pub(crate) fn token_done(&mut self) -> Result<(), ()> {
        self.token_done_inner()?;
        self.token.clean();
        self.values.clear();
        self.buf.clear();
        Ok(())
    }

    /// `lxb_html_tokenizer_state_token_text_done_m`.
    pub(crate) fn token_text_done(&mut self) -> Result<(), ()> {
        if self.token.text.is_some_and(|n| n != 0) {
            self.token_done_inner()?;
        }
        self.token.clean();
        self.values.clear();
        self.buf.clear();
        Ok(())
    }

    /// `lxb_html_tokenizer_state_token_emit_text_not_empty_m`.
    pub(crate) fn emit_text_not_empty(&mut self) -> Result<(), ()> {
        if !self.buf.is_empty() {
            self.token.tag_id = res::TAG__TEXT;
            self.set_text();
            self.token_done_inner()?;
            self.token.clean();
            self.values.clear();
        }
        Ok(())
    }

    /// `lxb_html_tokenizer_set_state_by_tag`: the state a start tag in the HTML
    /// namespace switches the tokenizer to.
    fn set_state_by_tag(&mut self, scripting: bool, tag_id: usize, ns: usize) {
        if ns != res::NS_HTML {
            self.state = data_before;
            return;
        }
        match tag_id {
            res::TAG_TITLE | res::TAG_TEXTAREA => {
                self.enter_text_content(TextContent::Rcdata, tag_id);
            }
            res::TAG_STYLE
            | res::TAG_XMP
            | res::TAG_IFRAME
            | res::TAG_NOEMBED
            | res::TAG_NOFRAMES => {
                self.enter_text_content(TextContent::Rawtext, tag_id);
            }
            res::TAG_SCRIPT => {
                self.enter_text_content(TextContent::ScriptData, tag_id);
            }
            res::TAG_NOSCRIPT => {
                if scripting {
                    self.enter_text_content(TextContent::Rawtext, tag_id);
                    return;
                }
                self.state = data_before;
            }
            res::TAG_PLAINTEXT => self.enter_plaintext(),
            _ => {}
        }
    }
}

/// Intern through the sink's names table. The sink is only away while it is
/// being handed a token, and no state interns from inside the sink.
#[inline]
fn intern(
    sink: &mut Option<&mut dyn TokenSink>,
    f: impl FnOnce(&mut Names) -> Result<usize, crate::html::names::NamesOom>,
) -> Result<usize, crate::html::names::NamesOom> {
    match sink {
        Some(s) => f(s.names()),
        None => Err(crate::html::names::NamesOom),
    }
}

/* ---- byte classes (lexbor/core/str_res.h) ---- */

#[inline]
pub(crate) fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

#[inline]
pub(crate) fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// `lexbor_str_data_ncasecmp_first`: compare `first` (a keyword) with `sec`
/// ASCII case-insensitively for `sec.len()` bytes or until the keyword ends.
/// `Some(rest of the keyword)` on a match so far (empty: the whole keyword
/// matched), `None` on a mismatch.
pub(crate) fn ncasecmp_first(first: &'static [u8], sec: &[u8]) -> Option<&'static [u8]> {
    for (i, &c) in sec.iter().enumerate() {
        let Some(&k) = first.get(i) else {
            return Some(&first[i..]);
        };
        if !k.eq_ignore_ascii_case(&c) {
            return None;
        }
    }
    Some(first.get(sec.len()..).unwrap_or_default())
}

/// `lexbor_swar_seek4`: skip whole machine words that hold none of the four
/// bytes; the caller's byte loop does the rest. Returns a position at or
/// before the first occurrence.
#[inline]
pub(crate) fn seek4(input: &[u8], mut data: usize, end: usize, c: [u8; 4]) -> usize {
    const ONES: u64 = u64::MAX / 0xFF;
    const HIGH: u64 = ONES * 0x80;
    let end = end.min(input.len());
    let has_zero = |v: u64| v.wrapping_sub(ONES) & !v & HIGH;
    while data + 8 <= end {
        let Some(chunk) = input.get(data..data + 8) else {
            break;
        };
        let mut word = [0u8; 8];
        word.copy_from_slice(chunk);
        let bytes = u64::from_le_bytes(word);
        let matches = has_zero(bytes ^ (ONES * c[0] as u64))
            | has_zero(bytes ^ (ONES * c[1] as u64))
            | has_zero(bytes ^ (ONES * c[2] as u64))
            | has_zero(bytes ^ (ONES * c[3] as u64));
        if matches != 0 {
            return data + (matches.trailing_zeros() / 8) as usize;
        }
        data += 8;
    }
    data
}
