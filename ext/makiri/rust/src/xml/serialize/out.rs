//! The output buffer and XML escaping, shared by both serializers.

#![forbid(unsafe_code)]

use crate::cbuf::Buf;
use crate::xml::model::{Document as XmlDoc, NodeId};

/// A write either succeeded or the output buffer refused it (its ceiling, or
/// OOM). The reason is the buffer's; the caller maps it to [`super::Failure`].
pub(super) type W = Result<(), ()>;

pub(super) fn put(b: &mut Buf, bytes: &[u8]) -> W {
    b.append(bytes).map_err(|_| ())
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
    put(b, b"<?")?;
    put(b, doc.span(doc.node(n).local))?;
    if doc.node(n).value.len != 0 {
        put(b, b" ")?;
        put(b, doc.span(doc.node(n).value))?;
    }
    put(b, b"?>")
}

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
