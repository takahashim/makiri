//! Which `:lexbor-contains()` arguments reach the vendored CSS parser is
//! Makiri's decision, not the parser's, and this module is where it is made -
//! for a selector and for a stylesheet alike, before either is parsed.
//!
//! [`neutralized`] rewrites the NAME of any `lexbor-contains(` function whose
//! argument it cannot positively recognise as one the parser accepts, turning
//! it into an unknown function OF THE SAME BYTE LENGTH. The parser then answers
//! as it answers any unknown pseudo function: a selector is a syntax error, and
//! a stylesheet rule becomes `bad_style` while the rest of the sheet survives -
//! rejecting the whole text would cost a caller every other rule in a `<style>`.
//! Byte length is preserved so `stylesheet.rs` can still resolve `selector_text`
//! against the caller's ORIGINAL bytes; nobody is shown the rewritten name.
//!
//! # The decision is made on Lexbor's own tokens
//!
//! The text is read by Lexbor's CSS syntax tokenizer ([`css_tokens`]) - the
//! code that feeds the parser, over the same bytes - not by a second reading
//! of the grammar. A hand-written scanner did that until it was found ending
//! strings where the tokenizer did not (CR, FF, backslash-newline) and giving
//! up on the rest of the text after an unterminated one, so a
//! `:lexbor-contains(#x)` after a bad string reached the parser unseen, and
//! `parse_stylesheet` died in `lxb_css_selector_serialize_escape_write`,
//! serializing the pseudo-class the failed argument parse had left behind.
//! What was wrong there was the READING. The hazard itself is in the selector
//! parser's `:lexbor-contains()` state and what it leaves on failure (the
//! v3.0.0 heap overflow was there too) - code the tokenizer never enters, so
//! running the tokenizer on its own over such text is safe.
//!
//! Every FUNCTION token whose decoded name is `lexbor-contains` (ASCII case
//! folded, as the parser's name lookup folds) is examined, whatever precedes
//! it - a superset of the pseudo-classes the parser builds. Its argument must
//! be exactly the parser's success path: `WS* (<string> | <ident>) WS*
//! (<ident "i"|"I"> WS?)? )`. Anything else, end of input included, gets the
//! name rewritten.
//!
//! # Three rules, all load-bearing
//!
//! **See every spelling the parser sees.** The tokenizer decodes escapes and
//! drops comments exactly as it does for the parser, so `:LEXBOR-CONTAINS(`,
//! `:\6C exbor-contains(` and a name after a bad string are all found. The
//! only shortcut is [`may_name_it`], which skips the tokenizer for text where
//! no `(` has the name, in any case, right before it or an escape in the span
//! a name could occupy - the only two ways to spell it. It over-approximates
//! by construction, and `lexbor/tests.rs` covers the widest spelling.
//!
//! **Never be LAXER than the parser.** Stricter only costs an exotic-but-valid
//! selector its match; laxer means the decision was not made at all.
//! `lexbor/tests.rs`'s `guard_agreement` pins that direction against the real
//! parser.
//!
//! **Never hand the original bytes on.** An allocation failure - of the copy,
//! or inside the tokenizer - means the decision could not be made, so
//! [`neutralized`] returns [`Oom`] and its callers report it instead of parsing
//! anyway.
//!
//! [`css_tokens`]: crate::lexbor::css_tokens

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use crate::falloc::try_to_vec;
use crate::lexbor::css_tokens::{tokens, Kind, Tok};

/// The pseudo-class this restricts, ASCII-lowercased.
const NAME: &[u8] = b"lexbor-contains";

/// The byte written over a neutralised name. Any run of these is a valid
/// identifier, and a run at least `NAME.len()` long matches no pseudo-class the
/// parser knows - which is the whole point.
const FILLER: u8 = b'z';

/// The decision could not be made: the copy, or the tokenizer, could not
/// allocate. A caller must NOT hand the original bytes on - see the module's
/// third rule.
#[derive(Debug, PartialEq, Eq)]
pub struct Oom;

/// `input` with every unrecognised `lexbor-contains(` renamed, or `None` when
/// there is nothing to rewrite.
pub fn neutralized(input: &[u8]) -> Result<Option<Vec<u8>>, Oom> {
    if !may_name_it(input) {
        return Ok(None);
    }
    let mut out: Option<Vec<u8>> = None;
    let mut oom = false;
    let mut on_bad = |start: usize, end: usize| {
        if out.is_none() {
            out = try_to_vec(input);
        }
        match out.as_mut().and_then(|o| o.get_mut(start..end)) {
            Some(span) => {
                span.fill(FILLER);
                ControlFlow::Continue(())
            }
            None => {
                oom = true;
                ControlFlow::Break(())
            }
        }
    };
    let mut guard = Guard::Idle;
    tokens(input, &mut |t| guard.step(t, &mut on_bad)).map_err(|_| Oom)?;
    /* The end of the input with an argument still open: the parser takes
     * that as the end of the function, but it is not the `)` we require. */
    if let Some((start, end)) = guard.open_name() {
        if on_bad(start, end).is_break() {
            oom = true;
        }
    }
    if oom {
        return Err(Oom);
    }
    Ok(out)
}

/// Whether `input` can hold a function token named [`NAME`]; when it cannot,
/// the tokenizer need not run. An over-approximation, and it has to be one:
/// `true` only costs a tokenizer pass, `false` skips the decision.
///
/// A function token is its name IMMEDIATELY followed by `(`, so every `(` is
/// examined. A name without an escape is literal - each character its own
/// source byte, and a non-ASCII byte folds to no ASCII letter - so it can only
/// be the [`NAME`]`.len()` bytes before the `(`. A name WITH an escape holds a
/// backslash inside its source span, which [`escape_before`] looks for.
fn may_name_it(input: &[u8]) -> bool {
    input.iter().enumerate().any(|(open, &b)| {
        b == b'('
            && ((open >= NAME.len() && input[open - NAME.len()..open].eq_ignore_ascii_case(NAME))
                || escape_before(input, open))
    })
}

/// The longest source span a [`NAME`]-long identifier can have: every
/// character an escape of `\`, six hex digits and a CRLF.
const MAX_NAME_SPAN: usize = NAME.len() * 9;

/// Whether an escaped identifier can end at `open`: walking back from it over
/// every byte an identifier's source can hold, a backslash is met.
///
/// The walk stops only at a byte no identifier span can contain - not a name
/// byte, not the character an escape covers (the byte after a backslash), not
/// the whitespace ending a hex escape (after a hex digit, or LF after CR) - or
/// after [`MAX_NAME_SPAN`] bytes. So it never stops inside a real name's span,
/// and a backslash in that span is always met.
fn escape_before(input: &[u8], open: usize) -> bool {
    let floor = open.saturating_sub(MAX_NAME_SPAN);
    let mut at = open;
    while at > floor {
        let b = input[at - 1];
        let prev = at.checked_sub(2).map(|p| input[p]);
        if b == b'\\' || prev == Some(b'\\') {
            return true;
        }
        let ends_hex_escape = matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
            && prev.is_some_and(|p| p.is_ascii_hexdigit() || p == b'\r');
        if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80 || ends_hex_escape) {
            return false;
        }
        at -= 1;
    }
    false
}

/// Whether `bytes` - text serialized from the rewritten buffer - can hold a
/// rewritten name. Every rewrite leaves a run of at least `NAME.len()`
/// [`FILLER`] bytes, so text without one reads exactly as the caller wrote it,
/// and `stylesheet.rs` takes a declaration value from the original only when
/// it has one.
pub fn may_hold_rewrite(bytes: &[u8]) -> bool {
    let mut run = 0;
    for &b in bytes {
        run = if b == FILLER { run + 1 } else { 0 };
        if run >= NAME.len() {
            return true;
        }
    }
    false
}

/// Where the reading of one `lexbor-contains(` argument stands. Each open
/// state carries the `[start, end)` of the function's name.
#[derive(Clone, Copy)]
enum Guard {
    Idle,
    /// Past the `(`: whitespace, then a string or an ident.
    Before(usize, usize),
    /// Past the argument: whitespace, then the flag or `)`.
    After(usize, usize),
    /// Past the flag: at most ONE whitespace token, then `)` - the parser
    /// takes one whitespace there, not a run (a comment splits a run in two).
    Flag(usize, usize, bool),
}

impl Guard {
    fn open_name(self) -> Option<(usize, usize)> {
        match self {
            Guard::Idle => None,
            Guard::Before(s, e) | Guard::After(s, e) | Guard::Flag(s, e, _) => Some((s, e)),
        }
    }

    /// Feed one token; `on_bad` is told each name to rewrite.
    fn step(
        &mut self,
        t: Tok<'_>,
        on_bad: &mut dyn FnMut(usize, usize) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        let ws = t.kind == Kind::Whitespace;
        let next = match *self {
            Guard::Idle => Some(Guard::Idle),
            Guard::Before(s, e) if ws => Some(Guard::Before(s, e)),
            Guard::Before(s, e) if matches!(t.kind, Kind::String | Kind::Ident) => {
                Some(Guard::After(s, e))
            }
            Guard::After(s, e) if ws => Some(Guard::After(s, e)),
            Guard::After(s, e) if t.kind == Kind::Ident && (t.value == b"i" || t.value == b"I") => {
                Some(Guard::Flag(s, e, false))
            }
            Guard::Flag(s, e, false) if ws => Some(Guard::Flag(s, e, true)),
            Guard::After(..) | Guard::Flag(..) if t.kind == Kind::RParen => Some(Guard::Idle),
            _ => None,
        };
        let now = match next {
            /* Recognised so far; a token that closes an argument is spent. */
            Some(g) if !matches!(*self, Guard::Idle) => {
                *self = g;
                return ControlFlow::Continue(());
            }
            Some(_) => Guard::Idle,
            /* Not the parser's success path: rewrite the name, then read this
             * same token afresh - it may open another `lexbor-contains(`. */
            None => {
                if let Some((s, e)) = self.open_name() {
                    on_bad(s, e)?;
                }
                Guard::Idle
            }
        };
        *self = now;
        if t.kind == Kind::Function && t.value.eq_ignore_ascii_case(NAME) {
            *self = Guard::Before(t.start, name_end(t));
        }
        ControlFlow::Continue(())
    }
}

/// The end of a function token's name: its `(`, which `css_tokens` checks is
/// the token's last byte.
fn name_end(t: Tok<'_>) -> usize {
    t.end.saturating_sub(1).max(t.start)
}
