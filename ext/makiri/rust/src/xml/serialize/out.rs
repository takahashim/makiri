//! The output buffer and XML escaping, shared by both serializers.

#![forbid(unsafe_code)]

use super::Failure;
use crate::cbuf::{Buf, BufError};
use crate::xml::model::{Document as XmlDoc, NodeId};

/// A write either succeeded or failed for the reason it carries: the failure
/// is decided where it happens - the buffer's ceiling or OOM here, a spent
/// step budget or an unbound prefix in the scope - and travels up with `?`, so
/// no writer has to work out afterwards why it stopped.
pub(super) type W = Result<(), Failure>;

pub(super) fn put(b: &mut Buf, bytes: &[u8]) -> W {
    b.append(bytes).map_err(|e| match e {
        BufError::Limit => Failure::OutputCap,
        BufError::Oom => Failure::Oom,
    })
}

/// A processing instruction: `<?target data?>`, with the space only when there
/// is data.
///
/// Neither form escapes or reformats a PI, so the rule is the same for both and
/// lives here rather than being spelled out in each.
///
/// Data that starts with whitespace does not survive a re-parse: XML reads
/// every space after the target as the separator (§2.6, `PITarget (S ...)`),
/// so `create_processing_instruction("t", "  x")` comes back as `x`. No
/// spelling of the PI keeps it, which is why this writes the data as it is
/// rather than refusing or altering it.
pub(super) fn put_pi(b: &mut Buf, doc: &XmlDoc, n: NodeId) -> W {
    let data = doc.span(doc.node(n).value);
    writable_chars(data)?;
    if data.windows(2).any(|w| w == b"?>") {
        return Err(Failure::UnwritableData);
    }
    put(b, b"<?")?;
    put(b, doc.span(doc.node(n).local))?;
    if doc.node(n).value.len != 0 {
        put(b, b" ")?;
        put(b, doc.span(doc.node(n).value))?;
    }
    put(b, b"?>")
}

/// A comment: `<!--data-->`, the same for both forms. Data holding `--`, or
/// ending with `-`, has no comment that parses (XML 1.0 §2.5), and no escape;
/// the DOM holds it (`createComment("a--b")`), so it is refused here.
pub(super) fn put_comment(b: &mut Buf, data: &[u8]) -> W {
    writable_chars(data)?;
    if data.last() == Some(&b'-') || data.windows(2).any(|w| w == b"--") {
        return Err(Failure::UnwritableData);
    }
    put(b, b"<!--")?;
    put(b, data)?;
    put(b, b"-->")
}

/// `s` as XML can hold it: every character an XML 1.0 `Char`, as the parser
/// requires. The DOM holds any string (`createTextNode("\f")`), so the
/// mutators take it, and data outside the class is refused where it would be
/// written - no character reference spells a C0 control or U+FFFE either.
pub(super) fn writable_chars(s: &[u8]) -> W {
    match s.iter().enumerate().any(|(i, &c)| non_xml_char_at(s, i, c)) {
        false => Ok(()),
        true => Err(Failure::UnwritableData),
    }
}

/// Whether the byte `c` at `i` of `s` starts a character XML 1.0's `Char`
/// excludes. The arena holds only valid UTF-8 (every string is verified on
/// the way in, and the parser validates what it reads), where the excluded
/// characters are exactly two byte patterns: a C0 control but tab, LF and CR,
/// and U+FFFE / U+FFFF (`EF BF BE` / `EF BF BF`) - a surrogate has no UTF-8
/// form. So a byte test, in the escaper's own loop, rather than a second pass
/// decoding every character (`validate_chars`), which made a text-heavy
/// `to_xml` 80% slower.
#[inline(always)]
fn non_xml_char_at(s: &[u8], i: usize, c: u8) -> bool {
    match c {
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F => true,
        0xEF => s.get(i + 1) == Some(&0xBF) && matches!(s.get(i + 2), Some(0xBE | 0xBF)),
        _ => false,
    }
}

/* `BYTE_CLASS`: what the escaper does with a byte before looking at it
 * closely. PLAIN is written as it is; SPECIAL may be escaped (`&`, `<`, `>`,
 * `"`, and tab / LF / CR, which are escaped in an attribute only); REFUSED is a
 * C0 control XML has no `Char` for; MAYBE_NONCHAR is `0xEF`, which starts
 * U+FFFE / U+FFFF or an ordinary character. */
const PLAIN: u8 = 0;
const SPECIAL: u8 = 1;
const REFUSED: u8 = 2;
const MAYBE_NONCHAR: u8 = 3;
const BYTE_CLASS: [u8; 256] = {
    let mut t = [PLAIN; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = REFUSED;
        i += 1;
    }
    t[b'\t' as usize] = SPECIAL;
    t[b'\n' as usize] = SPECIAL;
    t[b'\r' as usize] = SPECIAL;
    t[b'&' as usize] = SPECIAL;
    t[b'<' as usize] = SPECIAL;
    t[b'>' as usize] = SPECIAL;
    t[b'"' as usize] = SPECIAL;
    t[0xEF] = MAYBE_NONCHAR;
    t
};

/// Which characters an escaper replaces, and with what.
///
/// XML 1.0 and Canonical XML escape the same class of characters and differ
/// only in two details - whether `>` is escaped inside an attribute value, and
/// whether a character reference is decimal or hex - so this is one loop over a
/// table rather than two near-identical loops that could drift apart.
pub(super) struct Escape {
    /// `>` inside an attribute value. XML 1.0 escapes it everywhere; Canonical
    /// XML's minimal escaping only does so in character data.
    gt_in_attr: bool,
    tab: &'static [u8],
    lf: &'static [u8],
    cr: &'static [u8],
}

/// XML 1.0 escaping (decimal character references).
pub(super) const XML: Escape = Escape {
    gt_in_attr: true,
    tab: b"&#9;",
    lf: b"&#10;",
    cr: b"&#13;",
};

/// Canonical XML 1.0 escaping (hex character references, `>` kept in attributes).
pub(super) const C14N: Escape = Escape {
    gt_in_attr: false,
    tab: b"&#x9;",
    lf: b"&#xA;",
    cr: b"&#xD;",
};

impl Escape {
    /// Write `s`, replacing what this table names. Runs of ordinary bytes go out
    /// in one `append` each.
    pub(super) fn write(&self, b: &mut Buf, s: &[u8], attr: bool) -> W {
        let mut start = 0usize;
        for (i, &c) in s.iter().enumerate() {
            /* One table load per byte; only the rare bytes go further. What
             * XML cannot hold is refused here, in the one pass the escaper
             * makes anyway (`non_xml_char_at`). A test before the match cost a
             * text-heavy `to_xml` ~8%, arms inside it ~27%; the table is ~18%
             * FASTER than the match alone was, before any of this. */
            match BYTE_CLASS[c as usize] {
                PLAIN => continue,
                REFUSED => return Err(Failure::UnwritableData),
                MAYBE_NONCHAR if non_xml_char_at(s, i, c) => return Err(Failure::UnwritableData),
                MAYBE_NONCHAR => continue,
                _ => {}
            }
            let rep: &[u8] = match c {
                b'&' => b"&amp;",
                b'<' => b"&lt;",
                b'>' if !attr || self.gt_in_attr => b"&gt;",
                b'"' if attr => b"&quot;",
                b'\t' if attr => self.tab,
                b'\n' if attr => self.lf,
                b'\r' => self.cr,
                _ => continue,
            };
            if i > start {
                put(b, &s[start..i])?;
            }
            put(b, rep)?;
            start = i + 1;
        }
        if s.len() > start {
            put(b, &s[start..])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::non_xml_char_at;

    /// On valid UTF-8 the byte test is the XML `Char` class the parser checks
    /// (`validate_chars`), character for character.
    #[test]
    fn the_byte_test_is_the_xml_char_class_on_utf8() {
        let mut cps: Vec<u32> = (0..0x3000).collect();
        cps.extend([
            0xD7FF, 0xE000, 0xFFFC, 0xFFFD, 0xFFFE, 0xFFFF, 0x10000, 0x10FFFF,
        ]);
        for cp in cps {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf).as_bytes();
            let refused = s.iter().enumerate().any(|(i, &c)| non_xml_char_at(s, i, c));
            assert_eq!(refused, !crate::xml::chars::validate_chars(s), "U+{cp:04X}");
        }
    }
}
