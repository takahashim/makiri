//! A document parse from its bytes: the encoding decided by
//! `html::encoding`, the bytes decoded into UTF-8 text, and the parse
//! (`parse.rs`) run over it - again, once, if "change the encoding" asks.
//! The one part of the tree builder that reads a decoder, so the one built
//! only with `html-dom` (which brings `encoding_rs`).

use super::guard::DepthLimit;
use super::parse::{parse_html_probing, Utf8Text};
use super::{HtmlParseError, MetaProbe};
use crate::html::dom::HtmlParsed;
use crate::html::encoding::{
    decode, sniff, Decision, DocumentInput, Encoding, EncodingWatch, UTF_8,
};

/// Parse a document from its bytes (`input`): its encoding sniffed
/// (`html::encoding::sniff`), the bytes decoded, and the whole re-read once
/// if a `<meta>` met while the encoding is tentative changes it - the HTML
/// Standard's "change the encoding", which re-reads from the bytes since the
/// decoder cannot be switched on the fly. The document records the encoding
/// it was read in.
pub fn parse_document_bytes(
    input: DocumentInput<'_>,
    limit: DepthLimit,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let decision = sniff(input.bytes, input.transport);
    let mut watch = EncodingWatch::new(decision.encoding);
    let probe = decision
        .tentative
        .then_some(&mut watch as &mut (dyn MetaProbe + 'static));
    let first = parse_under(&input, &decision, limit, probe);
    let (mut parsed, encoding) = match watch.change() {
        /* Now certain: the first parse - which the tree builder stopped at
         * the `<meta>`, so its failure is that stop - is dropped, as a
         * browser drops the page it re-reads. */
        Some(changed) => {
            drop(first);
            let reread = parse_under(&input, &Decision::certain(changed, 0), limit, None)?;
            (reread, changed)
        }
        None => (first?, decision.encoding),
    };
    parsed.set_encoding(encoding.name());
    Ok(parsed)
}

/// Read `input` as `decision` says - its body decoded into text - and parse
/// it, telling `probe` of each `<meta>` while the decision is tentative.
fn parse_under(
    input: &DocumentInput<'_>,
    decision: &Decision,
    limit: DepthLimit,
    probe: Option<&mut (dyn MetaProbe + 'static)>,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let (body, known_valid) = decision.body(input);
    let text = Text::of(body, known_valid, decision.encoding)?;
    parse_html_probing(text.utf8(), limit, probe)
}

/// A document's text as the parser reads it: its own bytes when they are
/// read as UTF-8 (the parser's sanitiser replaces invalid sequences as the
/// UTF-8 decoder does), or another encoding's, decoded.
enum Text<'a> {
    Utf8(Utf8Text<'a>),
    Decoded(Vec<u8>),
}

impl<'a> Text<'a> {
    /// `bytes` (any BOM already taken off) as UTF-8 text under `encoding`;
    /// `known_valid` counts only for UTF-8 ([`Decision::body`]).
    ///
    /// [`Decision::body`]: crate::html::encoding::Decision::body
    fn of(
        bytes: &'a [u8],
        known_valid: bool,
        encoding: &'static Encoding,
    ) -> Result<Text<'a>, HtmlParseError> {
        if encoding == UTF_8 {
            return Ok(Text::Utf8(Utf8Text { bytes, known_valid }));
        }
        decode(bytes, encoding)
            .map(Text::Decoded)
            .ok_or(HtmlParseError::Failed)
    }

    fn utf8(&self) -> Utf8Text<'_> {
        match self {
            Text::Utf8(text) => *text,
            /* A decoder's output is valid UTF-8 by construction. */
            Text::Decoded(bytes) => Utf8Text {
                bytes,
                known_valid: true,
            },
        }
    }
}
