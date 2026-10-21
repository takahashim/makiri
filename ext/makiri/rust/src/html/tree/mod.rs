//! The HTML tree builder, ported from Lexbor's `html/tree.c` and
//! `html/tree/*.c`, writing straight into the arena DOM
//! (`crate::html::dom`).
//!
//! The port keeps Lexbor's control flow (`docs/rewrite_parser.ja.md`, stage
//! 4): one module per insertion mode, each function named after the Lexbor
//! function it ports, so that any difference from Lexbor's tree is a porting
//! error the differential tests catch. An insertion mode returns whether the
//! token was consumed (`true`) or is to be reprocessed (`false`), as Lexbor's
//! do, and the dispatcher loops until it is consumed.
//!
//! What changed, and only for ownership and safety (no behaviour changes):
//!
//! - **Nodes are [`NodeId`]s** of the arena being built, and the tree builder
//!   holds the document by `&mut` for the parse.
//! - **The insertion modes are a [`Mode`] enum**, not function pointers.
//! - **A token is a [`Tok`]**: the fields of Lexbor's `lxb_html_token_t` the
//!   tree builder reads or moves (its text bounds, `begin`, `base_element`),
//!   over the tokenizer's token, buffer and attribute values. Lexbor's
//!   on-stack fake tokens are `Tok`s with no attributes.
//! - **Every allocation is fallible** (`falloc`): a failure sets the status
//!   and stops, as Lexbor's do.
//! - **The marker of the active formatting list is `None`**, not a static
//!   element.
//! - **An attribute with no value is remembered in [`NodeState::valueless`]**:
//!   Lexbor keeps a NULL value for one (`<b x>`), which the Noah's Ark clause
//!   tells apart from an empty one (`<b x="">`), and the arena stores both
//!   as an empty span.

#![forbid(unsafe_code)]

mod actions;
use crate::compat::{self, Fix};
mod error;
mod fragment;
pub mod guard;
mod modes;
mod parse;
mod select;
mod stacks;
pub(crate) use stacks::Found;
mod stamp;

pub use error::{TreeError, TreeErrorId};
pub use fragment::{parse_fragment, ContextElement, ContextFacts, FragmentFailure, ParsedFragment};
pub use parse::{
    empty_html_document, parse_document, parse_html, parse_with_panicking_hook, HtmlParseError,
    ParseFailure, Parsed,
};

use crate::falloc::VecPush;
use crate::html::dom::{Document, NodeId};
use crate::html::names::Names;
use crate::html::res::names as res;
use crate::html::tokenizer::{flags, Stop, TokenAttr, TokenSink, Tokenizer};

/// Why the tree builder stopped (`tree->status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeStatus {
    Ok,
    /// `LXB_STATUS_ERROR_MEMORY_ALLOCATION`.
    Oom,
    /// `LXB_STATUS_ABORTED`: `lxb_html_tree_process_abort` with no other
    /// status set.
    Aborted,
    /// `LXB_STATUS_ERROR`.
    Error,
}

/// The arena could not allocate.
impl From<crate::html::dom::DomOom> for TreeStatus {
    #[inline]
    fn from(_: crate::html::dom::DomOom) -> TreeStatus {
        TreeStatus::Oom
    }
}

/// The names table could not allocate.
impl From<crate::html::names::NamesOom> for TreeStatus {
    #[inline]
    fn from(_: crate::html::names::NamesOom) -> TreeStatus {
        TreeStatus::Oom
    }
}

/// The insertion modes (`lxb_html_tree_insertion_mode_*`), including
/// Lexbor's three helper modes for the newline after `<pre>`, `<listing>`
/// and `<textarea>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    InBodySkipNewLine,
    InBodySkipNewLineTextarea,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// Where a node goes relative to the place found for it
/// (`lxb_html_tree_insertion_position_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ipos {
    Child,
    Before,
}

/// How a foreign element's attributes are adjusted as it is inserted: the
/// hook Lexbor sets on `tree->before_append_attr`, passed here as an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttrAdjust {
    /// `lxb_html_tree_adjust_attributes_mathml`.
    Mathml,
    /// `lxb_html_tree_adjust_attributes_svg`.
    Svg,
    /// Only "adjust foreign attributes": an element in a namespace that is
    /// neither MathML nor SVG, which only a fragment's context has (P19).
    Foreign,
}

/// The token the tree builder is processing: Lexbor's `lxb_html_token_t` as
/// far as the tree builder reads and changes it. Its text is
/// `[text_start, text_end)` of the tokenizer's token text, and its
/// attributes - when it has any - the tokenizer's token's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok {
    pub tag_id: usize,
    /// Lexbor's `type` ([`flags`]).
    pub flags: u8,
    pub begin: Option<usize>,
    pub end: Option<usize>,
    pub text_start: usize,
    pub text_end: usize,
    pub null_count: usize,
    /// `base_element`: the element whose attributes a recreated formatting
    /// element copies.
    pub base: Option<NodeId>,
    /// Whether the tokenizer's attributes belong to this token (a fake token
    /// has none).
    pub has_attrs: bool,
}

impl Tok {
    /// The tokenizer's current token.
    fn of(tkz: &Tokenizer<'_, '_>) -> Tok {
        let t = &tkz.token;
        Tok {
            tag_id: t.tag_id,
            flags: t.flags,
            begin: t.begin,
            end: t.end,
            text_start: 0,
            text_end: t.text.unwrap_or(0),
            null_count: t.null_count,
            base: None,
            has_attrs: true,
        }
    }

    /// `lxb_html_token_t fake_token = {0}; fake_token.tag_id = tag`.
    pub fn fake(tag_id: usize) -> Tok {
        Tok {
            tag_id,
            flags: 0,
            begin: None,
            end: None,
            text_start: 0,
            text_end: 0,
            null_count: 0,
            base: None,
            has_attrs: false,
        }
    }

    #[inline]
    pub fn is_close(&self) -> bool {
        self.flags & flags::CLOSE != 0
    }

    /// The token's attributes: none for a fake token, whatever the
    /// tokenizer holds.
    #[inline]
    pub fn attrs<'a>(&self, tkz: &'a Tokenizer<'_, '_>) -> &'a [TokenAttr] {
        if self.has_attrs {
            &tkz.token.attrs
        } else {
            &[]
        }
    }

    /// The token's text.
    #[inline]
    pub fn text<'a>(&self, tkz: &'a Tokenizer<'_, '_>) -> &'a [u8] {
        tkz.text()
            .get(self.text_start..self.text_end)
            .unwrap_or_default()
    }

    /// `lxb_html_token_data_skip_ws_begin`.
    pub fn skip_ws_begin(&mut self, tkz: &Tokenizer<'_, '_>) {
        let text = tkz.text();
        let mut at = self.text_start;
        while at < self.text_end && is_ws(text.get(at).copied().unwrap_or(0)) {
            at += 1;
        }
        self.begin = self.begin.map(|b| b + (at - self.text_start));
        self.text_start = at;
    }

    /// `lxb_html_token_data_skip_one_newline_begin`.
    pub fn skip_one_newline_begin(&mut self, tkz: &Tokenizer<'_, '_>) {
        if self.text_start < self.text_end && tkz.text().get(self.text_start) == Some(&b'\n') {
            self.begin = self.begin.map(|b| b + 1);
            self.text_start += 1;
        }
    }

    /// `lxb_html_token_data_split_ws_begin`: skip the leading whitespace,
    /// and return it as a token of its own (empty when there was none).
    pub fn split_ws_begin(&mut self, tkz: &Tokenizer<'_, '_>) -> Tok {
        let mut ws = *self;
        self.skip_ws_begin(tkz);
        if self.text_start == self.text_end {
            return ws;
        }
        if self.text_start == ws.text_start {
            /* memset(ws_token, 0) */
            return Tok::fake(0);
        }
        ws.end = self.begin;
        ws.text_end = self.text_start;
        ws
    }
}

/// The whitespace of the tokenizer's character map
/// (`LEXBOR_STR_RES_MAP_CHAR_WHITESPACE`): tab, LF, FF, CR, space.
#[inline]
pub(crate) fn is_ws(c: u8) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Pending table character tokens (`lxb_html_tree_pending_table_t`): each
/// one's text, copied, in `bytes`.
#[derive(Default)]
pub(crate) struct PendingTable {
    pub bytes: Vec<u8>,
    pub list: Vec<(usize, usize)>,
    pub have_non_ws: bool,
}

/// A set of nodes, by id.
#[derive(Default)]
pub(crate) struct NodeSet(Vec<u64>);

impl NodeSet {
    pub fn insert(&mut self, n: NodeId) -> Result<(), ()> {
        let w = n.to_word();
        let i = w / 64;
        while self.0.len() <= i {
            self.0.falloc_push(0)?;
        }
        self.0[i] |= 1 << (w % 64);
        Ok(())
    }

    pub fn remove(&mut self, n: NodeId) {
        let w = n.to_word();
        if let Some(b) = self.0.get_mut(w / 64) {
            *b &= !(1 << (w % 64));
        }
    }

    pub fn contains(&self, n: NodeId) -> bool {
        let w = n.to_word();
        self.0.get(w / 64).is_some_and(|b| b & (1 << (w % 64)) != 0)
    }
}

/// What Lexbor's DOM keeps on its nodes, and the arena does not, that the
/// tree builder reads back: which attributes were made with no value (see
/// the module doc), and which options are selected (`select.rs`).
#[derive(Default)]
pub(crate) struct NodeState {
    /// The attributes made with no value.
    pub(crate) valueless: NodeSet,
    /// The options whose selectedness is true.
    pub(crate) selected: NodeSet,
}

/// `lxb_html_tree_t`, building into one arena document.
pub struct TreeBuilder<'d> {
    doc: &'d mut Document,
    /// The fragment parsing algorithm's context element.
    fragment: Option<NodeId>,
    /// The context element is an HTML integration point, though it was made
    /// with no attributes (`ContextFacts::html_integration_point`).
    context_integration_point: bool,
    /// The form element pointer.
    form: Option<NodeId>,
    /// The head element pointer (Lexbor's `document->head`).
    head: Option<NodeId>,
    open: stacks::OpenElements,
    afe: stacks::ActiveFormatting,
    template_modes: Vec<Mode>,
    pending: PendingTable,
    errors: Vec<TreeError>,
    foster_parenting: bool,
    frameset_ok: bool,
    scripting: bool,
    mode: Mode,
    original_mode: Mode,
    status: TreeStatus,
    /// What Lexbor's DOM keeps on its nodes and the arena does not.
    node_state: NodeState,
    /// The parse guard (`guard.rs`).
    guard: guard::Guard,
    /// Whether elements get their start tag's offset (`stamp.rs`).
    stamping: stamp::Stamping,
}

impl<'d> TreeBuilder<'d> {
    /// `lxb_html_tree_create` + `_init` + `_begin`, over `doc`, held to
    /// `guard`. Every parse starts here, through [`for_document`] or
    /// `for_fragment` (`fragment.rs`), which set what the parse needs before
    /// the first token.
    ///
    /// [`for_document`]: Self::for_document
    fn new(doc: &'d mut Document, guard: guard::Guard) -> TreeBuilder<'d> {
        TreeBuilder {
            doc,
            fragment: None,
            context_integration_point: false,
            form: None,
            head: None,
            open: stacks::OpenElements::default(),
            afe: stacks::ActiveFormatting::default(),
            template_modes: Vec::new(),
            pending: PendingTable::default(),
            errors: Vec::new(),
            foster_parenting: false,
            frameset_ok: true,
            scripting: false,
            mode: Mode::Initial,
            original_mode: Mode::Initial,
            status: TreeStatus::Ok,
            node_state: NodeState::default(),
            guard,
            stamping: stamp::Stamping::default(),
        }
    }

    /// A document parse into `doc`, held to `guard` and stamped as
    /// `stamping` says.
    pub(crate) fn for_document(
        doc: &'d mut Document,
        guard: guard::Guard,
        stamping: stamp::Stamping,
    ) -> TreeBuilder<'d> {
        TreeBuilder {
            stamping,
            ..TreeBuilder::new(doc, guard)
        }
    }

    /// What the guard refused the parse for, if it did.
    pub(crate) fn guard_stopped(&self) -> Option<guard::GuardStop> {
        self.guard.stopped
    }

    /// The tree's parse errors.
    pub fn errors(&self) -> &[TreeError] {
        &self.errors
    }

    /// Why the tree builder stopped, if it did.
    pub fn status(&self) -> TreeStatus {
        self.status
    }

    /// The namespace of the adjusted current node, for the tokenizer's CDATA
    /// test (`lxb_html_tokenizer_current_namespace`).
    pub(crate) fn current_namespace(&self) -> usize {
        match self.adjusted_current_node() {
            Some(n) => self.ns(n),
            None => res::NS__UNDEF,
        }
    }

    /* ---- node reads (Lexbor's node->local_name / node->ns) ---- */

    #[inline]
    pub(crate) fn tag(&self, n: NodeId) -> usize {
        self.doc.local(n) as usize
    }

    #[inline]
    pub(crate) fn ns(&self, n: NodeId) -> usize {
        self.doc.ns(n) as usize
    }

    /// `lxb_html_tree_node_is`.
    #[inline]
    pub(crate) fn node_is(&self, n: NodeId, tag: usize) -> bool {
        self.tag(n) == tag && self.ns(n) == res::NS_HTML
    }

    /// Whether a form start or end tag sees a template: the HTML Standard's
    /// "parsing template contents" - a `template` on the stack of open
    /// elements, or a fragment parsed in a `template` context - where Lexbor
    /// asks only the first (docs/parser_differences.ja.md P2).
    pub(crate) fn form_in_template(&self) -> bool {
        if self
            .open_find_reverse(res::TAG_TEMPLATE, res::NS_HTML)
            .is_some()
        {
            return true;
        }
        compat::fixed(Fix::P2)
            && self
                .fragment
                .is_some_and(|f| self.node_is(f, res::TAG_TEMPLATE))
    }

    /// `lxb_html_tree_current_node`.
    #[inline]
    pub(crate) fn current_node(&self) -> Option<NodeId> {
        self.open.last().copied()
    }

    /// `lxb_html_tree_adjusted_current_node`.
    #[inline]
    pub(crate) fn adjusted_current_node(&self) -> Option<NodeId> {
        if self.fragment.is_some() && self.open.len() == 1 {
            return self.fragment;
        }
        self.current_node()
    }

    /// `lxb_html_tree_process_abort`.
    pub(crate) fn process_abort(&mut self) -> bool {
        if self.status == TreeStatus::Ok {
            self.status = TreeStatus::Aborted;
        }
        self.open_abandon();
        true
    }

    /// Set the status to `status` and abort. [`OrAbort`] is the form for a
    /// step's `Result`.
    pub(crate) fn abort_with(&mut self, status: TreeStatus) -> bool {
        self.status = status;
        self.process_abort()
    }

    /// `lxb_html_tree_parse_error`.
    pub(crate) fn parse_error(&mut self, tok: &Tok, id: TreeErrorId) {
        let _ = self.errors.falloc_push(TreeError {
            id,
            begin: tok.begin,
            end: tok.end,
        });
    }

    /// `lxb_html_tree_construction_dispatcher`.
    fn dispatch(&mut self, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
        let Some(adjusted) = self.adjusted_current_node() else {
            return self.run(self.mode, tkz, tok);
        };
        if self.ns(adjusted) == res::NS_HTML {
            return self.run(self.mode, tkz, tok);
        }
        if self.mathml_text_integration_point(adjusted) {
            if !tok.is_close() && tok.tag_id != res::TAG_MGLYPH && tok.tag_id != res::TAG_MALIGNMARK
            {
                return self.run(self.mode, tkz, tok);
            }
            if tok.tag_id == res::TAG__TEXT {
                return self.run(self.mode, tkz, tok);
            }
        }
        if self.tag(adjusted) == res::TAG_ANNOTATION_XML
            && self.ns(adjusted) == res::NS_MATH
            && !tok.is_close()
            && tok.tag_id == res::TAG_SVG
        {
            return self.run(self.mode, tkz, tok);
        }
        if self.html_integration_point(adjusted)
            && (!tok.is_close() || tok.tag_id == res::TAG__TEXT)
        {
            return self.run(self.mode, tkz, tok);
        }
        if tok.tag_id == res::TAG__END_OF_FILE {
            return self.run(self.mode, tkz, tok);
        }
        modes::foreign_content::foreign_content(self, tkz, tok)
    }

    /// Run the insertion mode `mode` (`tree->mode(tree, token)`).
    pub(crate) fn run(&mut self, mode: Mode, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
        use modes::*;
        match mode {
            Mode::Initial => initial::initial(self, tkz, tok),
            Mode::BeforeHtml => before_html::before_html(self, tkz, tok),
            Mode::BeforeHead => before_head::before_head(self, tkz, tok),
            Mode::InHead => in_head::in_head(self, tkz, tok),
            Mode::InHeadNoscript => in_head_noscript::in_head_noscript(self, tkz, tok),
            Mode::AfterHead => after_head::after_head(self, tkz, tok),
            Mode::InBody => in_body::in_body(self, tkz, tok),
            Mode::InBodySkipNewLine => in_body::skip_new_line(self, tkz, tok),
            Mode::InBodySkipNewLineTextarea => in_body::skip_new_line_textarea(self, tkz, tok),
            Mode::Text => text::text(self, tkz, tok),
            Mode::InTable => in_table::in_table(self, tkz, tok),
            Mode::InTableText => in_table_text::in_table_text(self, tkz, tok),
            Mode::InCaption => in_caption::in_caption(self, tkz, tok),
            Mode::InColumnGroup => in_column_group::in_column_group(self, tkz, tok),
            Mode::InTableBody => in_table_body::in_table_body(self, tkz, tok),
            Mode::InRow => in_row::in_row(self, tkz, tok),
            Mode::InCell => in_cell::in_cell(self, tkz, tok),
            Mode::InTemplate => in_template::in_template(self, tkz, tok),
            Mode::AfterBody => after_body::after_body(self, tkz, tok),
            Mode::InFrameset => in_frameset::in_frameset(self, tkz, tok),
            Mode::AfterFrameset => after_frameset::after_frameset(self, tkz, tok),
            Mode::AfterAfterBody => after_after_body::after_after_body(self, tkz, tok),
            Mode::AfterAfterFrameset => after_after_frameset::after_after_frameset(self, tkz, tok),
        }
    }

    /// `lxb_html_tree_insertion_mode`: process one token, reprocessing until
    /// a mode consumes it.
    pub(crate) fn process(&mut self, tkz: &mut Tokenizer<'_, '_>) -> TreeStatus {
        let mut tok = Tok::of(tkz);
        /* The guard reads the token as the tokenizer handed it over. */
        let handed = tok;
        let before = self.stamp_before(&handed);
        /* An aborted parse has emptied the stack: reprocessing would only
         * fail again, so a token is never retried once the status is set. */
        while !self.dispatch(tkz, &mut tok) && self.status == TreeStatus::Ok {}
        tkz.set_current_namespace(self.current_namespace());
        self.stamp_after(before);
        if self.status == TreeStatus::Ok && !self.guard_after(&handed) {
            self.status = TreeStatus::Error;
        }
        self.status
    }
}

/// A failed step aborts the parse with its own status: Lexbor's
/// `tree->status = status; return lxb_html_tree_process_abort(tree)`, the
/// one way a mode handles a failure. `None` means the parse is aborted, and
/// the mode returns `true` (the token consumed).
pub(crate) trait OrAbort<T> {
    fn or_abort(self, tree: &mut TreeBuilder<'_>) -> Option<T>;
}

impl<T> OrAbort<T> for Result<T, TreeStatus> {
    #[inline]
    fn or_abort(self, tree: &mut TreeBuilder<'_>) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(st) => {
                tree.abort_with(st);
                None
            }
        }
    }
}

impl TokenSink for TreeBuilder<'_> {
    /// `lxb_html_tree_token_callback`.
    fn token(&mut self, tkz: &mut Tokenizer<'_, '_>) -> Result<(), Stop> {
        match self.process(tkz) {
            TreeStatus::Ok => Ok(()),
            TreeStatus::Oom => Err(Stop::Oom),
            _ => Err(Stop::Error),
        }
    }

    fn names(&mut self) -> &mut Names {
        &mut self.doc.names
    }
}
