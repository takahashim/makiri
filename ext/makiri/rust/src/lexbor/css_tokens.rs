//! Lexbor's own CSS syntax tokenizer, run over a byte slice on its own.
//!
//! `contains_guard` used to read the text with a hand-written scanner, and the
//! scanner and the tokenizer disagreed about where a string ends (CR and FF end
//! one too; backslash-newline continues it; the tokenizer resumes after a bad
//! string, the scanner stopped). Every disagreement was a `:lexbor-contains()`
//! the guard never saw. So the guard reads the tokens the parser reads,
//! produced by the same code over the same bytes: this module is the only
//! place that touches the tokenizer, and it hands out nothing but a safe
//! [`Tok`] per token.
//!
//! The parsers never switch the tokenizer's one mode (`with_unicode_range` is
//! false everywhere in Lexbor) and drop comments as this does, so the stream
//! is theirs token for token.

#![allow(unsafe_code)]

use core::ops::ControlFlow;
use core::ptr::NonNull;

use crate::lexbor::abi::consts::STATUS_OK;
use crate::lexbor::abi::{
    lxb_css_syntax_token, lxb_css_syntax_token_consume,
    lxb_css_syntax_token_type_t_LXB_CSS_SYNTAX_TOKEN_FUNCTION as FUNCTION,
    lxb_css_syntax_token_type_t_LXB_CSS_SYNTAX_TOKEN__END as END,
    lxb_css_syntax_token_type_t_LXB_CSS_SYNTAX_TOKEN__EOF as EOF, lxb_css_syntax_tokenizer_clean,
    lxb_css_syntax_tokenizer_create, lxb_css_syntax_tokenizer_destroy,
    lxb_css_syntax_tokenizer_init, lxb_css_syntax_tokenizer_t,
};

/// The token kinds the guard tells apart; everything else is [`Kind::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Function,
    Other,
}

/// One token: its kind, the `[start, end)` byte span it was read from (after
/// any comment the tokenizer dropped in front of it), and - for a function -
/// its DECODED name, escapes resolved, without the `(`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok<'a> {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub value: &'a [u8],
}

/// The tokenizer could not run to the end: Lexbor could not allocate, or it
/// reported a span outside the input (never observed; refused all the same,
/// since a guard that cannot place a token cannot decide).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Failed;

/// A created, initialised tokenizer, destroyed however its scope exits.
struct Tokenizer(NonNull<lxb_css_syntax_tokenizer_t>);

impl Tokenizer {
    fn new() -> Option<Tokenizer> {
        // SAFETY: the constructor pair Lexbor documents; `init` runs on exactly
        // what `create` returned, and a half-initialised one is destroyed by
        // `Drop` (Lexbor's destroy tolerates the pieces `init` did not reach).
        unsafe {
            let this = Tokenizer(NonNull::new(lxb_css_syntax_tokenizer_create())?);
            (lxb_css_syntax_tokenizer_init(this.0.as_ptr()) == STATUS_OK).then_some(this)
        }
    }
}

impl Drop for Tokenizer {
    fn drop(&mut self) {
        // SAFETY: this type owns the tokenizer, and nothing else destroys it.
        unsafe { lxb_css_syntax_tokenizer_destroy(self.0.as_ptr()) };
    }
}

/// A tokenizer kept past its call only while its scratch buffer is at most
/// this large: the buffer grows to the longest token read and never shrinks.
const KEEP_SCRATCH: usize = 64 * 1024;

thread_local! {
    /// One tokenizer per thread, reused: building one is five allocations,
    /// which cost as much as tokenizing a selector. Per THREAD rather than
    /// behind the GVL because it holds nothing Ruby or any document can see.
    static SPARE: core::cell::Cell<Option<Tokenizer>> = const { core::cell::Cell::new(None) };
}

/// Run Lexbor's tokenizer over `input`, handing each token to `each` in order
/// until the end of the input or until `each` breaks. Comments are dropped, as
/// every Lexbor parser drops them.
pub(crate) fn tokens(
    input: &[u8],
    each: &mut dyn FnMut(Tok<'_>) -> ControlFlow<()>,
) -> Result<(), Failed> {
    /* Taken out of the slot, so a re-entrant call (there is none) or a panic
     * in `each` cannot leave two users on one tokenizer: the panic drops it. */
    let tkz = match SPARE.with(core::cell::Cell::take) {
        Some(t) => t,
        None => Tokenizer::new().ok_or(Failed)?,
    };
    let walked = run(&tkz, input, each);
    // SAFETY: `tkz` is live and ours. `clean` empties its token pool and text
    // arena and forgets the input, which `run` no longer reads.
    let scratch = unsafe {
        let raw = tkz.0.as_ptr();
        lxb_css_syntax_tokenizer_clean(raw);
        ((*raw).end as usize).wrapping_sub((*raw).start as usize)
    };
    /* A failed walk may have left it anywhere; a big scratch buffer is not
     * worth keeping. Either way it is dropped - destroyed - here. */
    if walked.is_ok() && scratch <= KEEP_SCRATCH {
        SPARE.with(|s| s.set(Some(tkz)));
    }
    walked
}

fn run(
    tkz: &Tokenizer,
    input: &[u8],
    each: &mut dyn FnMut(Tok<'_>) -> ControlFlow<()>,
) -> Result<(), Failed> {
    let raw = tkz.0.as_ptr();
    let base = input.as_ptr();
    // SAFETY: `raw` is live and ours. These three stores are what Lexbor's
    // inline `lxb_css_syntax_tokenizer_buffer_set` does; the tokenizer only
    // reads `[in_begin, in_end)`, which is `input`, borrowed for this call.
    unsafe {
        (*raw).in_begin = base;
        (*raw).in_p = base;
        (*raw).in_end = base.wrapping_add(input.len());
    }
    loop {
        // SAFETY: `raw` is live; the token it returns belongs to the tokenizer
        // and stays valid - value bytes included - until the consume below.
        let tok = unsafe { lxb_css_syntax_token(raw) };
        if tok.is_null() {
            return Err(Failed);
        }
        // SAFETY: non-null, and live until consumed.
        let t = unsafe { &*tok };
        if t.type_ == EOF || t.type_ == END {
            return Ok(());
        }
        let kind = if t.type_ == FUNCTION {
            Kind::Function
        } else {
            Kind::Other
        };
        // SAFETY: `base` is the member every token carries first; the tokenizer
        // sets it for every token it produces.
        let b = unsafe { t.types.base };
        let start = (b.begin as usize).wrapping_sub(base as usize);
        /* `offset` is where the tokenizer began this token, comments dropped
         * in front of it included; `length` runs from there to its end. */
        let end = t.offset.checked_add(b.length).ok_or(Failed)?;
        if start > end || end > input.len() {
            return Err(Failed);
        }
        /* A function token ends with the `(` it consumed; the guard rewrites
         * the name in front of it and relies on that. */
        if kind == Kind::Function && (end == start || input.get(end - 1) != Some(&b'(')) {
            return Err(Failed);
        }
        let value: &[u8] = match kind {
            Kind::Function => {
                // SAFETY: for a function the union holds a string token,
                // whose `data`/`length` Lexbor has just written.
                let s = unsafe { t.types.string };
                if s.data.is_null() {
                    &[]
                } else {
                    // SAFETY: Lexbor keeps `length` bytes at `data` until the
                    // token is consumed, which is after `each` returns.
                    unsafe { core::slice::from_raw_parts(s.data, s.length) }
                }
            }
            _ => &[],
        };
        let flow = each(Tok {
            kind,
            start,
            end,
            value,
        });
        // SAFETY: `raw` is live; this frees the token read above, which
        // nothing refers to any more.
        unsafe { lxb_css_syntax_token_consume(raw) };
        if flow.is_break() {
            return Ok(());
        }
    }
}
