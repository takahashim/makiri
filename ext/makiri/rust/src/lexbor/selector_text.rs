//! A parsed selector written back as text - the `:text` of each selector
//! `Makiri::Lexbor::CSS.parse_stylesheet` returns - following CSSOM's
//! "serialize a selector": every identifier through "serialize an
//! identifier" and every string through "serialize a string", so the text
//! reads back as the selector it came from.
//!
//! Lexbor's own `lxb_css_selector_serialize_chain` did this until it was
//! found writing identifiers as their DECODED value: `.md\:block` came back
//! as `.md:block` (a class and a pseudo-class), `.a\,b` as `.a,b` (two
//! selectors), and an attribute value's `\` or newline went out raw. A
//! caller that re-parses the text - dommy builds its cascade that way -
//! dropped the rule or, worse, matched something else. It also wrote
//! `:current(S)` as `:current()`. Lexbor is not patched; its serializer is
//! simply not called.
//!
//! The layout is Lexbor's (`#b > span`, `, ` between alternatives, `odd` /
//! `even`, lower-case pseudo names), so selectors without escapes read as
//! they did. What this cannot write faithfully - a pseudo-element function,
//! whose argument Lexbor does not keep, an attribute in the `*` namespace
//! (see `simple`), or a kind the typed view does not know - is
//! [`Fail::Lossy`], never a wrong text: the stylesheet reader reports that
//! rule as `:bad_style` with the caller's own prelude.
//!
//! Nested lists (`:is()`, `:not()`, `:where()`, `:has()`, `:current()`,
//! `of S`) are walked on a heap work list, not by recursion, for the reason
//! `css_match::compile` gives: a selector nested a few thousand deep must
//! not overflow a small Fiber's stack.

#![forbid(unsafe_code)]

use core::ffi::c_long;

use crate::falloc::{OomResult, Reserve, VecPush};
use crate::lexbor::css_parser::{
    AttrMatch, CaseModifier, Combinator, FunctionArg, List, Lists, Selector, Simple,
};

/// Why a selector was not written.
#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    Oom,
    /// The parse holds something this cannot write back as it was written.
    Lossy,
}

impl crate::falloc::Oom for Fail {
    #[inline]
    fn oom() -> Self {
        Fail::Oom
    }
}

/// One comma alternative of a selector list, as text, appended to `out`.
pub fn write(list: List<'_>, out: &mut Vec<u8>) -> Result<(), Fail> {
    let mut work: Vec<Work<'_>> = Vec::new();
    if let Some(first) = list.first() {
        work.falloc_push(Work::Chain {
            sel: first,
            lead: true,
        })
        .or_oom()?;
    }
    while let Some(w) = work.pop() {
        match w {
            Work::Close => put(out, b")")?,
            Work::Lists { mut lists, lead } => {
                let Some(l) = lists.next() else { continue };
                if !lead {
                    put(out, b", ")?;
                }
                work.falloc_push(Work::Lists { lists, lead: false })
                    .or_oom()?;
                if let Some(sel) = l.first() {
                    work.falloc_push(Work::Chain { sel, lead: true }).or_oom()?;
                }
            }
            Work::Chain { sel, lead } => {
                combinator(out, sel.combinator(), lead)?;
                let nested = simple(out, sel)?;
                // LIFO: the nested list, then `)`, then the rest of the chain.
                if let Some(next) = sel.next() {
                    work.falloc_push(Work::Chain {
                        sel: next,
                        lead: false,
                    })
                    .or_oom()?;
                }
                if let Some(lists) = nested {
                    work.falloc_push(Work::Close).or_oom()?;
                    work.falloc_push(Work::Lists { lists, lead: true })
                        .or_oom()?;
                }
            }
        }
    }
    Ok(())
}

/// What is left to write, most recent first.
enum Work<'p> {
    /// A chain from this simple selector on; `lead` when it begins the chain.
    Chain { sel: Selector<'p>, lead: bool },
    /// A list's comma alternatives from here on; `lead` before the first.
    Lists { lists: Lists<'p>, lead: bool },
    /// The `)` closing a functional pseudo-class whose list was written.
    Close,
}

/// The combinator in front of a simple selector. A chain's first selector
/// carries one only in a relative selector (`:has(> a)`), where whitespace
/// is the default and not written.
fn combinator(out: &mut Vec<u8>, c: Combinator, lead: bool) -> Result<(), Fail> {
    let sym: &[u8] = match c {
        Combinator::Close => return Ok(()),
        Combinator::Descendant => {
            return if lead { Ok(()) } else { put(out, b" ") };
        }
        Combinator::Child => b">",
        Combinator::NextSibling => b"+",
        Combinator::SubsequentSibling => b"~",
        Combinator::Column => b"||",
        Combinator::Other => return Err(Fail::Lossy),
    };
    if !lead {
        put(out, b" ")?;
    }
    put(out, sym)?;
    put(out, b" ")
}

/// One simple selector. Returns the selector list it opens - its `(` and
/// everything before the list already written - for the caller to write
/// and close.
fn simple<'p>(out: &mut Vec<u8>, s: Selector<'p>) -> Result<Option<Lists<'p>>, Fail> {
    match s.simple() {
        Simple::Universal => {
            namespace(out, s)?;
            put(out, b"*")?;
        }
        Simple::Type => {
            namespace(out, s)?;
            ident(out, s.name())?;
        }
        Simple::Id => {
            put(out, b"#")?;
            ident(out, s.name())?;
        }
        Simple::Class => {
            put(out, b".")?;
            ident(out, s.name())?;
        }
        Simple::Attribute(at) => {
            // Lexbor stores `*` for `[|a]` (no namespace) as well as for an
            // escaped `\*|` prefix, and rejects `[*|a]` itself, so `*` here
            // cannot be written back as any of them.
            if s.ns() == Some(b"*") {
                return Err(Fail::Lossy);
            }
            put(out, b"[")?;
            namespace(out, s)?;
            ident(out, s.name())?;
            if let Some(value) = at.value {
                put(
                    out,
                    match at.op {
                        AttrMatch::Equal => b"=",
                        AttrMatch::Include => b"~=",
                        AttrMatch::Dash => b"|=",
                        AttrMatch::Prefix => b"^=",
                        AttrMatch::Suffix => b"$=",
                        AttrMatch::Substring => b"*=",
                        AttrMatch::Other => return Err(Fail::Lossy),
                    },
                )?;
                string(out, value)?;
                match at.case {
                    CaseModifier::Unset => {}
                    CaseModifier::Insensitive => put(out, b" i")?,
                    CaseModifier::Sensitive => put(out, b" s")?,
                }
            }
            put(out, b"]")?;
        }
        Simple::PseudoClass(_) => {
            put(out, b":")?;
            pseudo_name(out, s.name())?;
        }
        Simple::PseudoClassFunction(arg) => {
            put(out, b":")?;
            pseudo_name(out, s.name())?;
            put(out, b"(")?;
            match arg {
                FunctionArg::Selectors { lists, .. } => return Ok(Some(lists)),
                FunctionArg::Nth { anb: Some(n), .. } => {
                    anb(out, n.a, n.b)?;
                    if let Some(of) = n.of {
                        put(out, b" of ")?;
                        return Ok(Some(of));
                    }
                }
                FunctionArg::Nth { anb: None, .. } => return Err(Fail::Lossy),
                FunctionArg::Contains(Some(c)) => {
                    string(out, c.needle)?;
                    if c.insensitive {
                        put(out, b" i")?;
                    }
                }
                FunctionArg::Contains(None) => return Err(Fail::Lossy),
                FunctionArg::Other => match s.current_arg() {
                    Some(lists) => return Ok(Some(lists)),
                    None => return Err(Fail::Lossy),
                },
            }
            put(out, b")")?;
        }
        Simple::PseudoElement => {
            if s.is_pseudo_element_function() {
                return Err(Fail::Lossy);
            }
            put(out, b"::")?;
            pseudo_name(out, s.name())?;
        }
        Simple::Other => return Err(Fail::Lossy),
    }
    Ok(None)
}

/// A pseudo-class or pseudo-element name, in the lower case Lexbor's own name
/// table spells it: it matched one of those ASCII names ignoring case, so
/// `:HOVER` is written `:hover`, as Lexbor's serializer wrote it.
fn pseudo_name(out: &mut Vec<u8>, name: &[u8]) -> Result<(), Fail> {
    let start = out.len();
    ident(out, name)?;
    out[start..].make_ascii_lowercase();
    Ok(())
}

/// `ns|` when a namespace was written: `|` alone for no namespace, `*|` for
/// any. On a type selector `*` is taken as the any-namespace wildcard,
/// although an escaped prefix (`\*|a`) is stored the same way: refusing it
/// would refuse every `*|a`, and a prefix named `*` needs an `@namespace`
/// that declares one.
fn namespace(out: &mut Vec<u8>, s: Selector<'_>) -> Result<(), Fail> {
    let Some(ns) = s.ns() else { return Ok(()) };
    if ns == b"*" {
        put(out, b"*")?;
    } else {
        ident(out, ns)?;
    }
    put(out, b"|")
}

/// `<an+b>` in CSS Syntax's form, except that `2n+1` and `2n` are `odd` and
/// `even`, as Lexbor writes them.
fn anb(out: &mut Vec<u8>, a: c_long, b: c_long) -> Result<(), Fail> {
    match (a, b) {
        (2, 1) => return put(out, b"odd"),
        (2, 0) => return put(out, b"even"),
        (0, _) => return int(out, b),
        (1, _) => put(out, b"n")?,
        (-1, _) => put(out, b"-n")?,
        _ => {
            int(out, a)?;
            put(out, b"n")?;
        }
    }
    if b > 0 {
        put(out, b"+")?;
    }
    if b != 0 {
        int(out, b)?;
    }
    Ok(())
}

fn int(out: &mut Vec<u8>, v: c_long) -> Result<(), Fail> {
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let mut n = v.unsigned_abs();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if v < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    put(out, &buf[i..])
}

/// CSSOM's "serialize an identifier", over UTF-8: bytes from 0x80 up are
/// parts of non-ASCII code points, which an identifier takes as they are.
pub fn ident(out: &mut Vec<u8>, s: &[u8]) -> Result<(), Fail> {
    for (i, &c) in s.iter().enumerate() {
        match c {
            0 => put(out, REPLACEMENT)?,
            0x01..=0x1F | 0x7F => hex(out, c)?,
            b'0'..=b'9' if i == 0 || (i == 1 && s[0] == b'-') => hex(out, c)?,
            b'-' if i == 0 && s.len() == 1 => put(out, b"\\-")?,
            b'-' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | 0x80.. => put(out, &[c])?,
            _ => put(out, &[b'\\', c])?,
        }
    }
    Ok(())
}

/// CSSOM's "serialize a string": double-quoted, `"` and `\` escaped, control
/// characters as code points.
pub fn string(out: &mut Vec<u8>, s: &[u8]) -> Result<(), Fail> {
    put(out, b"\"")?;
    for &c in s {
        match c {
            0 => put(out, REPLACEMENT)?,
            0x01..=0x1F | 0x7F => hex(out, c)?,
            b'"' | b'\\' => put(out, &[b'\\', c])?,
            _ => put(out, &[c])?,
        }
    }
    put(out, b"\"")
}

/// U+FFFD, which CSS reads a NUL as.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// "Escape a character as code point": `\`, lower-case hex, a space.
fn hex(out: &mut Vec<u8>, c: u8) -> Result<(), Fail> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let hi = DIGITS[usize::from(c >> 4)];
    let lo = DIGITS[usize::from(c & 0xF)];
    if c >> 4 == 0 {
        put(out, &[b'\\', lo, b' '])
    } else {
        put(out, &[b'\\', hi, lo, b' '])
    }
}

fn put(out: &mut Vec<u8>, s: &[u8]) -> Result<(), Fail> {
    out.falloc_reserve(s.len()).or_oom()?;
    out.extend_from_slice(s);
    Ok(())
}
