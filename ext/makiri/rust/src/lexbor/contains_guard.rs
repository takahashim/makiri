//! Which `:lexbor-contains()` arguments reach the vendored CSS parser is
//! Makiri's decision, not the parser's, and this module is where it is made -
//! for a selector and for a stylesheet alike, before either is parsed.
//!
//! [`neutralized`] rewrites the NAME of any `:lexbor-contains` whose argument it
//! cannot positively recognise as one the parser accepts, turning it into an
//! unknown pseudo-class OF THE SAME BYTE LENGTH. The parser then answers as it
//! answers any unknown pseudo function: a selector is a syntax error, and a
//! stylesheet rule becomes `bad_style` while the rest of the sheet survives -
//! rejecting the whole text would cost a caller every other rule in a `<style>`.
//! Byte length is preserved so `stylesheet.rs` can still resolve `selector_text`
//! against the caller's ORIGINAL bytes; nobody is shown the rewritten name.
//!
//! # Three rules, all load-bearing
//!
//! **Decode identifier escapes.** The parser decodes them before matching, so a
//! plain substring search would miss spellings it still resolves to this
//! pseudo-class - `:LEXBOR-CONTAINS(` and every escaped form included.
//!
//! **Never be LAXER than the parser.** Stricter only costs an exotic-but-valid
//! selector its match; laxer means the decision was not made at all.
//! `lexbor/tests.rs`'s `guard_agreement` pins that direction against the real
//! parser.
//!
//! **Never hand the original bytes on.** An allocation failure means the
//! decision could not be made, so [`neutralized`] returns [`Oom`] and its
//! callers report a failed parse instead of parsing anyway.

#![forbid(unsafe_code)]

use crate::falloc::try_to_vec;

/// The pseudo-class this restricts, ASCII-lowercased.
const NAME: &[u8] = b"lexbor-contains";

/// The byte written over a neutralised name. Any run of these is a valid
/// identifier, and a run at least `NAME.len()` long matches no pseudo-class the
/// parser knows - which is the whole point.
const FILLER: u8 = b'z';

/// The copy could not be allocated. A caller must NOT hand the original bytes
/// on - see the module's third rule.
#[derive(Debug, PartialEq, Eq)]
pub struct Oom;

/// `input` with every unrecognised `:lexbor-contains` renamed, or `None` when
/// there is nothing to rewrite.
pub fn neutralized(input: &[u8]) -> Result<Option<Vec<u8>>, Oom> {
    if !scan(input, &mut |_, _| {}) {
        return Ok(None);
    }
    let mut out = try_to_vec(input).ok_or(Oom)?;
    /* Scanning twice keeps the common path - no match - free of the span list
     * an all-at-once pass would need. */
    scan(input, &mut |start, end| {
        for b in &mut out[start..end] {
            *b = FILLER;
        }
    });
    Ok(Some(out))
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

/// Walk `input`, reporting the `[start, end)` of each `:lexbor-contains` name
/// whose argument is not recognised. Returns whether any was reported.
fn scan(input: &[u8], on_bad: &mut dyn FnMut(usize, usize)) -> bool {
    let mut i = 0;
    let mut found = false;
    while i < input.len() {
        match input[i] {
            b'/' if input.get(i + 1) == Some(&b'*') => i = skip_comment(input, i),
            b'"' | b'\'' => i = skip_string(input, i),
            /* An escape outside a string still hides the byte after it. */
            b'\\' => i = skip_escape(input, i),
            b':' => {
                let start = i + 1;
                let (end, matches) = read_name(input, start);
                if matches && input.get(end) == Some(&b'(') && !argument_ok(input, end + 1) {
                    found = true;
                    on_bad(start, end);
                }
                /* `end == start` for a bare `:`; always move on. */
                i = if end > start { end } else { i + 1 };
            }
            _ => i += 1,
        }
    }
    found
}

/// The end of the identifier at `i`, and whether it decodes to [`NAME`] under
/// ASCII case folding. A non-identifier gives `(i, false)`.
fn read_name(input: &[u8], i: usize) -> (usize, bool) {
    let mut at = i;
    let mut decoded = 0usize;
    let mut matches = true;
    while at < input.len() {
        let (next, ch) = match input[at] {
            b'\\' => {
                let next = skip_escape(input, at);
                if next == at + 1 {
                    /* a trailing backslash is not an identifier character */
                    break;
                }
                (next, decode_escape(&input[at..next]))
            }
            b if is_name_byte(b) => (at + 1, Some(u32::from(b))),
            _ => break,
        };
        /* Compare as we go, and keep walking even once it cannot match: the end
         * of the identifier is what tells the caller where to resume. */
        match ch {
            Some(c) if decoded < NAME.len() && eq_ascii_fold(c, NAME[decoded]) => {}
            _ => matches = false,
        }
        decoded += 1;
        at = next;
    }
    (at, matches && decoded == NAME.len() && at > i)
}

/// Whether the argument list starting at `i` (just past the `(`) is one the
/// parser accepts: `WS* (<string> | <ident>) WS* ((i|I) WS*)? )`.
fn argument_ok(input: &[u8], i: usize) -> bool {
    let mut at = skip_blank(input, i);
    at = match input.get(at) {
        Some(b'"' | b'\'') => match string_end(input, at) {
            Some(end) => end,
            None => return false,
        },
        Some(_) => {
            /* An identifier, not just identifier BYTES: `123` is a number and
             * `.x` a delimiter, and the parser takes neither. */
            if !starts_ident(input, at) {
                return false;
            }
            let end = ident_end(input, at);
            if end == at {
                return false;
            }
            end
        }
        None => return false,
    };
    at = skip_blank(input, at);
    /* The optional case-insensitivity flag, a one-character identifier. */
    if let Some(&b) = input.get(at) {
        if b == b'i' || b == b'I' {
            let end = ident_end(input, at);
            if end == at + 1 {
                at = skip_blank(input, end);
            }
        }
    }
    input.get(at) == Some(&b')')
}

/* ------------------------------------------------------------------ *
 * the small pieces                                                   *
 * ------------------------------------------------------------------ */

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

/// Whether an identifier starts at `i`: a letter, `_`, a non-ASCII byte or an
/// escape, or a `-` before one of those (or before a second `-`). A digit does
/// not start one, which is what keeps `123` a number.
fn starts_ident(input: &[u8], i: usize) -> bool {
    let head = |at: usize| {
        input.get(at).is_some_and(|b| {
            b.is_ascii_alphabetic()
                || *b == b'_'
                || *b >= 0x80
                || (*b == b'\\' && at + 1 < input.len())
        })
    };
    if head(i) {
        return true;
    }
    input.get(i) == Some(&b'-') && (input.get(i + 1) == Some(&b'-') || head(i + 1))
}

fn eq_ascii_fold(c: u32, want: u8) -> bool {
    u8::try_from(c).is_ok_and(|b| b.eq_ignore_ascii_case(&want))
}

/// The index just past the escape at `i`. A lone trailing `\` yields `i + 1`,
/// which the callers read as "not an escape".
fn skip_escape(input: &[u8], i: usize) -> usize {
    let mut at = i + 1;
    if at >= input.len() {
        return at;
    }
    if !input[at].is_ascii_hexdigit() {
        /* `\` + one character; a newline does not escape. */
        return if input[at] == b'\n' { i + 1 } else { at + 1 };
    }
    let limit = core::cmp::min(at + 6, input.len());
    while at < limit && input[at].is_ascii_hexdigit() {
        at += 1;
    }
    /* One whitespace may terminate a hex escape; it is consumed with it. */
    if input.get(at).is_some_and(|b| is_blank_byte(*b)) {
        at += 1;
    }
    at
}

/// The code point an escape spells, or `None` when it is out of range. Only
/// used to compare against ASCII, so anything large may fail.
fn decode_escape(esc: &[u8]) -> Option<u32> {
    let body = esc.get(1..)?;
    if body.first().is_some_and(u8::is_ascii_hexdigit) {
        let mut value: u32 = 0;
        for b in body.iter().take_while(|b| b.is_ascii_hexdigit()) {
            value = value.checked_mul(16)?.checked_add(u32::from(hex(*b)))?;
        }
        Some(value)
    } else {
        body.first().map(|b| u32::from(*b))
    }
}

fn hex(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => b - b'A' + 10,
    }
}

fn ident_end(input: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < input.len() {
        match input[at] {
            b'\\' => {
                let next = skip_escape(input, at);
                if next == at + 1 {
                    break;
                }
                at = next;
            }
            b if is_name_byte(b) => at += 1,
            _ => break,
        }
    }
    at
}

/// The index just past the closing quote of the string at `i`, or `None` when
/// it is unterminated - which the parser rejects, so the caller must too.
fn string_end(input: &[u8], i: usize) -> Option<usize> {
    let quote = input[i];
    let mut at = i + 1;
    while at < input.len() {
        match input[at] {
            b'\\' => at = skip_escape(input, at),
            b'\n' => return None,
            b if b == quote => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

fn skip_string(input: &[u8], i: usize) -> usize {
    string_end(input, i).unwrap_or(input.len())
}

fn skip_comment(input: &[u8], i: usize) -> usize {
    let mut at = i + 2;
    while at + 1 < input.len() {
        if input[at] == b'*' && input[at + 1] == b'/' {
            return at + 2;
        }
        at += 1;
    }
    input.len()
}

fn is_blank_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

/// Whitespace, plus comments: the tokenizer drops those, so a selector may
/// carry one anywhere whitespace is allowed.
fn skip_blank(input: &[u8], i: usize) -> usize {
    let mut at = i;
    loop {
        while input.get(at).is_some_and(|b| is_blank_byte(*b)) {
            at += 1;
        }
        if input.get(at) == Some(&b'/') && input.get(at + 1) == Some(&b'*') {
            at = skip_comment(input, at);
            continue;
        }
        return at;
    }
}
