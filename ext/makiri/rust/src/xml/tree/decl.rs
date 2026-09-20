//! The XML declaration, `<?xml version="1.0" encoding="..." standalone="..."?>`.
//!
//! Its own module for the same reason [`super::dtd`] is: it builds NO node. All
//! it leaves behind is `Document::mark_encoding_decl`, which the serializer reads
//! to decide whether to write an `encoding` back out. The three pseudo-attribute
//! value grammars below are naming rules for nothing else, so they live with the
//! only production that uses them.

#![forbid(unsafe_code)]

use super::cursor::{Cursor, R};
use crate::xml::Document;

fn is_version_num(s: &[u8]) -> bool {
    s.len() >= 3 && s.starts_with(b"1.") && s[2..].iter().all(|b| b.is_ascii_digit())
}

fn is_enc_name(s: &[u8]) -> bool {
    match s.first() {
        Some(c0) if c0.is_ascii_alphabetic() => s[1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-'),
        _ => false,
    }
}

fn is_yes_no(s: &[u8]) -> bool {
    s == b"yes" || s == b"no"
}

fn decl_eq(cur: &mut Cursor<'_>) -> R {
    cur.skip_ws();
    if cur.peek() != Some(b'=') {
        return cur.syntax();
    }
    cur.advance();
    cur.skip_ws();
    Ok(())
}

fn decl_value(cur: &mut Cursor<'_>, ok: fn(&[u8]) -> bool) -> R {
    let v = cur.parse_quoted()?;
    if !ok(cur.slice(v)) {
        return cur.syntax();
    }
    Ok(())
}

/// '<?xml' consumed. version (encoding)? (standalone)? S? '?>'
pub(super) fn parse_body(cur: &mut Cursor<'_>, doc: &mut Document) -> R {
    cur.require_space()?;
    if !cur.eat_keyword(b"version") {
        return cur.syntax();
    }
    decl_eq(cur)?;
    let ver = cur.parse_quoted()?;
    /* §2.8: any 1.x. A 1.0 processor reads a 1.x document as 1.0, so one
     * that uses a 1.1-only feature fails on that feature, not its label. */
    if !is_version_num(cur.slice(ver)) {
        return cur.syntax();
    }
    let (mut saw_enc, mut saw_sd) = (false, false);
    loop {
        let had_s = cur.skip_some_ws();
        if cur.starts(b"?>") {
            cur.advance_n(2);
            return Ok(());
        }
        if !had_s {
            return cur.syntax();
        }
        if !saw_enc && !saw_sd && cur.eat_keyword(b"encoding") {
            saw_enc = true;
            doc.mark_encoding_decl();
            decl_eq(cur)?;
            decl_value(cur, is_enc_name)?;
        } else if !saw_sd && cur.eat_keyword(b"standalone") {
            saw_sd = true;
            decl_eq(cur)?;
            decl_value(cur, is_yes_no)?;
        } else {
            return cur.syntax();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_enc_name, is_version_num, is_yes_no};

    /// The declaration grammars, at their boundary forms.
    #[test]
    fn declaration_grammars_reject_boundary_forms() {
        assert!(is_version_num(b"1.0"));
        assert!(!is_version_num(b"1."));
        assert!(is_enc_name(b"UTF-8"));
        assert!(!is_enc_name(b"8UTF"));
        assert!(is_yes_no(b"yes"));
        assert!(is_yes_no(b"no"));
        assert!(!is_yes_no(b"Yes"));
    }
}
