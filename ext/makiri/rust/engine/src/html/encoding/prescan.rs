//! "Prescan a byte stream to determine its encoding" (HTML Standard), with
//! the "get an attribute" and "get an XML encoding" it uses, and the
//! algorithm for extracting a character encoding from a `<meta>` element's
//! `content`: the Standard's steps, read in place over the input's bytes.
//!
//! No step copies a name or value: the Standard lowercases them, and every
//! comparison here is ASCII case-insensitive instead.

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

use super::{declared, for_label};

/// How far the prescan reads ("User agents are encouraged to only prescan the
/// first 1024 bytes").
const PRESCAN_LEN: usize = 1024;

fn is_space(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// "Prescan a byte stream to determine its encoding", over its first
/// [`PRESCAN_LEN`] bytes. Running out of bytes aborts it, and then the
/// answer is "get an XML encoding" of the same bytes.
pub fn prescan(input: &[u8]) -> Option<&'static Encoding> {
    let s = input.get(..PRESCAN_LEN).unwrap_or(input);
    match prescan_meta(s) {
        Some(encoding) => Some(encoding),
        None => xml_encoding(s),
    }
}

/// The prescan's own steps: `None` where it runs out of bytes.
fn prescan_meta(s: &[u8]) -> Option<&'static Encoding> {
    if s.starts_with(&[0x3C, 0x00, 0x3F, 0x00, 0x78, 0x00]) {
        return Some(UTF_16LE);
    }
    if s.starts_with(&[0x00, 0x3C, 0x00, 0x3F, 0x00, 0x78]) {
        return Some(UTF_16BE);
    }
    let mut pos = 0;
    loop {
        let rest = s.get(pos..).filter(|r| !r.is_empty())?;
        if rest.starts_with(b"<!--") {
            /* The first `>` preceded by `--`, after the `<`; the dashes may
             * be the comment opener's own. */
            pos = (pos + 4..s.len()).find(|&j| s[j] == b'>' && s[j - 2..j] == *b"--")?;
        } else if rest.len() >= 6
            && rest[0] == b'<'
            && rest[1..5].eq_ignore_ascii_case(b"meta")
            && (is_space(rest[5]) || rest[5] == b'/')
        {
            pos += 5;
            if let Some(encoding) = meta_declaration(s, &mut pos)? {
                return Some(encoding);
            }
        } else if rest[0] == b'<' && {
            let at = if rest.get(1) == Some(&b'/') { 2 } else { 1 };
            rest.get(at).is_some_and(u8::is_ascii_alphabetic)
        } {
            pos = (pos..s.len()).find(|&j| is_space(s[j]) || s[j] == b'>')?;
            while get_attribute(s, &mut pos)?.is_some() {}
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") || rest.starts_with(b"<?") {
            pos = (pos + 1..s.len()).find(|&j| s[j] == b'>')?;
        }
        pos += 1;
    }
}

/// A `<meta>`'s attributes, from just after its name: the encoding it
/// declares, `Some(None)` when it declares none, `None` on running out of
/// bytes.
fn meta_declaration(s: &[u8], pos: &mut usize) -> Option<Option<&'static Encoding>> {
    /* Only these three names matter, so the attribute list is three flags. */
    let (mut seen_http_equiv, mut seen_content, mut seen_charset) = (false, false, false);
    let mut got_pragma = false;
    let mut need_pragma: Option<bool> = None;
    /* `None` is the Standard's null; `Some(None)` its failure. */
    let mut charset: Option<Option<&'static Encoding>> = None;
    while let Some((name, value)) = get_attribute(s, pos)? {
        if name.eq_ignore_ascii_case(b"http-equiv") {
            if !core::mem::replace(&mut seen_http_equiv, true)
                && value.eq_ignore_ascii_case(b"content-type")
            {
                got_pragma = true;
            }
        } else if name.eq_ignore_ascii_case(b"content") {
            if !core::mem::replace(&mut seen_content, true) {
                if let Some(encoding) = from_meta_content(value) {
                    if charset.is_none() {
                        charset = Some(Some(encoding));
                        need_pragma = Some(true);
                    }
                }
            }
        } else if name.eq_ignore_ascii_case(b"charset")
            && !core::mem::replace(&mut seen_charset, true)
        {
            charset = Some(for_label(value));
            need_pragma = Some(false);
        }
    }
    /* Processing. */
    match need_pragma {
        None => return Some(None),
        Some(true) if !got_pragma => return Some(None),
        _ => {}
    }
    Some(match charset {
        Some(Some(encoding)) => Some(declared(encoding)),
        _ => None,
    })
}

/// The prescan's "get an attribute": `Some(None)` when there is none (a
/// `>`), `None` on running out of bytes. The name and value are slices of
/// the input, compared ASCII case-insensitively by the caller rather than
/// lowercased into a copy.
fn get_attribute<'s>(s: &'s [u8], pos: &mut usize) -> Option<Option<(&'s [u8], &'s [u8])>> {
    while is_space(*s.get(*pos)?) || s[*pos] == b'/' {
        *pos += 1;
    }
    if s[*pos] == b'>' {
        return Some(None);
    }
    let name_start = *pos;
    loop {
        match *s.get(*pos)? {
            b'=' if *pos > name_start => {
                let name = &s[name_start..*pos];
                *pos += 1;
                return attribute_value(s, pos, name).map(Some);
            }
            b if is_space(b) => break,
            b'/' | b'>' => return Some(Some((&s[name_start..*pos], b""))),
            _ => *pos += 1,
        }
    }
    let name = &s[name_start..*pos];
    /* Spaces. */
    while is_space(*s.get(*pos)?) {
        *pos += 1;
    }
    if s[*pos] != b'=' {
        return Some(Some((name, b"")));
    }
    *pos += 1;
    attribute_value(s, pos, name).map(Some)
}

/// "Get an attribute" from its step labeled value, `name` read.
fn attribute_value<'s>(
    s: &'s [u8],
    pos: &mut usize,
    name: &'s [u8],
) -> Option<(&'s [u8], &'s [u8])> {
    while is_space(*s.get(*pos)?) {
        *pos += 1;
    }
    match s[*pos] {
        quote @ (b'"' | b'\'') => {
            let start = *pos + 1;
            let end = (start..s.len()).find(|&j| s[j] == quote)?;
            *pos = end + 1;
            Some((name, &s[start..end]))
        }
        b'>' => Some((name, b"")),
        _ => {
            let start = *pos;
            *pos += 1;
            while !(is_space(*s.get(*pos)?) || s[*pos] == b'>') {
                *pos += 1;
            }
            Some((name, &s[start..*pos]))
        }
    }
}

/// "Get an XML encoding": the `encoding` of an XML declaration at the very
/// start of `s`, the prescan's answer when it finds no `<meta>`.
fn xml_encoding(s: &[u8]) -> Option<&'static Encoding> {
    if !s.starts_with(b"<?xml") {
        return None;
    }
    let end = s.iter().position(|&b| b == b'>')?;
    let decl = &s[..end];
    let mut pos = decl.windows(8).position(|w| w == b"encoding")? + 8;
    let skip = |pos: &mut usize| {
        while s.get(*pos).is_some_and(|&b| b <= 0x20) {
            *pos += 1;
        }
    };
    skip(&mut pos);
    if s.get(pos) != Some(&b'=') {
        return None;
    }
    pos += 1;
    skip(&mut pos);
    let quote = *s.get(pos).filter(|&&q| q == b'"' || q == b'\'')?;
    pos += 1;
    let close = pos + s.get(pos..)?.iter().position(|&b| b == quote)?;
    let potential = &s[pos..close];
    if potential.iter().any(|&b| b <= 0x20) {
        return None;
    }
    let encoding = for_label(potential)?;
    Some(if encoding == UTF_16BE || encoding == UTF_16LE {
        UTF_8
    } else {
        encoding
    })
}

/// The algorithm for extracting a character encoding from a `<meta>`
/// element's `content` value.
pub fn from_meta_content(s: &[u8]) -> Option<&'static Encoding> {
    let mut pos = 0;
    loop {
        pos += s
            .get(pos..)?
            .windows(7)
            .position(|w| w.eq_ignore_ascii_case(b"charset"))?
            + 7;
        while s.get(pos).copied().is_some_and(is_space) {
            pos += 1;
        }
        if s.get(pos) != Some(&b'=') {
            continue;
        }
        pos += 1;
        while s.get(pos).copied().is_some_and(is_space) {
            pos += 1;
        }
        return match *s.get(pos)? {
            quote @ (b'"' | b'\'') => {
                let rest = &s[pos + 1..];
                let end = rest.iter().position(|&b| b == quote)?;
                for_label(&rest[..end])
            }
            _ => {
                let rest = &s[pos..];
                let end = rest
                    .iter()
                    .position(|&b| is_space(b) || b == b';')
                    .unwrap_or(rest.len());
                for_label(&rest[..end])
            }
        };
    }
}
