//! No `:lexbor-contains()` reaches the vendored CSS parser - for a selector
//! or a stylesheet alike - and this module is where that is made true, before
//! either is parsed.
//!
//! `:lexbor-contains()` is Lexbor's own extension, not CSS. Makiri does not
//! support it: its argument parser is the code that carried the v3.0.0 heap
//! overflow, and a failed argument parse left a pseudo-class behind that the
//! selector serializer then dereferenced. Rather than decide which arguments
//! are safe to hand that code, none is: [`neutralized`] rewrites the NAME of
//! every `lexbor-contains(` function, turning it into an unknown function OF
//! THE SAME BYTE LENGTH. The parser then answers as it answers any unknown
//! pseudo function: a selector is a syntax error, and a stylesheet rule
//! becomes `bad_style` (or loses the alternative, inside a forgiving list)
//! while the rest of the sheet survives - rejecting the whole text would cost
//! a caller every other rule in a `<style>`. Byte length is preserved so
//! `stylesheet.rs` can still resolve `selector_text` against the caller's
//! ORIGINAL bytes; nobody is shown the rewritten name.
//!
//! # The name is found on Lexbor's own tokens
//!
//! The text is read by Lexbor's CSS syntax tokenizer ([`css_tokens`]) - the
//! code that feeds the parser, over the same bytes - not by a second reading
//! of the grammar. A hand-written scanner did that until it was found ending
//! strings where the tokenizer did not (CR, FF, backslash-newline) and giving
//! up on the rest of the text after an unterminated one, so a
//! `:lexbor-contains(#x)` after a bad string reached the parser unseen, and
//! `parse_stylesheet` died in `lxb_css_selector_serialize_escape_write`. The
//! hazard is in the selector parser's `:lexbor-contains()` state - code the
//! tokenizer never enters, so running the tokenizer on its own over such text
//! is safe.
//!
//! Every FUNCTION token whose decoded name is `lexbor-contains` (ASCII case
//! folded, as the parser's name lookup folds) is rewritten, whatever precedes
//! it and whatever follows - a superset of the pseudo-classes the parser
//! builds.
//!
//! # Two rules, both load-bearing
//!
//! **See every spelling the parser sees.** The tokenizer decodes escapes and
//! drops comments exactly as it does for the parser, so `:LEXBOR-CONTAINS(`,
//! `:\6C exbor-contains(` and a name after a bad string are all found. The
//! only shortcut is [`may_name_it`], which skips the tokenizer for text where
//! no `(` has the name, in any case, right before it or an escape in the span
//! a name could occupy - the only two ways to spell it. It over-approximates
//! by construction, and `lexbor/tests.rs` covers the widest spelling, and
//! checks against the real parser that nothing it lets through is read as
//! the pseudo-class.
//!
//! **Never hand the original bytes on.** An allocation failure - of the copy,
//! or inside the tokenizer - means the name could not be looked for, so
//! [`neutralized`] returns [`Oom`] and its callers report it instead of parsing
//! anyway.
//!
//! [`css_tokens`]: crate::lexbor::css_tokens

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use crate::falloc::try_to_vec;
use crate::lexbor::css_tokens::{tokens, Kind, Tok};

/// The pseudo-class this removes, ASCII-lowercased.
const NAME: &[u8] = b"lexbor-contains";

/// The byte written over a neutralised name. Any run of these is a valid
/// identifier, and a run at least `NAME.len()` long matches no pseudo-class the
/// parser knows - which is the whole point.
const FILLER: u8 = b'z';

/// The name could not be looked for: the copy, or the tokenizer, could not
/// allocate. A caller must NOT hand the original bytes on - see the module's
/// second rule.
#[derive(Debug, PartialEq, Eq)]
pub struct Oom;

/// `input` with every `lexbor-contains(` renamed, or `None` when there is
/// nothing to rewrite.
pub fn neutralized(input: &[u8]) -> Result<Option<Vec<u8>>, Oom> {
    if !may_name_it(input) {
        return Ok(None);
    }
    let mut out: Option<Vec<u8>> = None;
    let mut oom = false;
    tokens(input, &mut |t| {
        if t.kind != Kind::Function || !t.value.eq_ignore_ascii_case(NAME) {
            return ControlFlow::Continue(());
        }
        if out.is_none() {
            out = try_to_vec(input);
        }
        match out.as_mut().and_then(|o| o.get_mut(t.start..name_end(t))) {
            Some(span) => {
                span.fill(FILLER);
                ControlFlow::Continue(())
            }
            None => {
                oom = true;
                ControlFlow::Break(())
            }
        }
    })
    .map_err(|_| Oom)?;
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

/// The end of a function token's name: its `(`, which `css_tokens` checks is
/// the token's last byte.
fn name_end(t: Tok<'_>) -> usize {
    t.end.saturating_sub(1).max(t.start)
}
