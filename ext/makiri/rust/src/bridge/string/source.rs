//! A Ruby String as parser input: [`HtmlSource`] (fragments, and the bytes
//! a document parse copies) and [`DocumentSource`] (a document parse, with
//! the transport layer's encoding).

#![forbid(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{Error, RString};

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;

use super::*;

/// A Ruby String as HTML parser input, under the text-input contract: its
/// encoding honoured ([`ruby_to_utf8_value`]), its bytes copied out, and
/// whether they are already known to be valid UTF-8, so the parser can skip
/// its sanitisation.
///
/// The one place that turns a Ruby String into bytes an HTML parser reads.
/// Both HTML entry points - a document parse and a fragment parse - take their
/// input through it, so the engine below never sees a `VALUE`. The copy is
/// what a document parse needs anyway, to read the source with the GVL
/// released.
pub struct HtmlSource {
    bytes: OwnedBuf,
    known_valid: bool,
}

impl HtmlSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(s: RString) -> Result<HtmlSource, Error> {
        let src = ruby_to_utf8_value(s)?;
        /* A transcode replaced every invalid or unmappable byte, so its result
         * is valid UTF-8 whatever its coderange says. */
        let transcoded = src.as_raw() != s.as_raw();
        let known_valid = transcoded || ruby_str_known_valid_utf8(src);
        let bytes = ruby_string_bytes(src)?;
        Ok(HtmlSource { bytes, known_valid })
    }

    /// Whether the bytes are valid UTF-8 already - never a scan, only what
    /// Ruby (or the transcode) has established.
    pub fn known_valid(&self) -> bool {
        self.known_valid
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// The bytes, owned - for a parse that runs with the GVL released.
    pub fn into_bytes(self) -> OwnedBuf {
        self.bytes
    }
}

/// A Ruby String as a DOCUMENT parse's input (`html::encoding`): its bytes
/// as they are, and the transport layer's encoding they come under, decided
/// by `html::encoding::transport_for` from `given` (`encoding:`) and the
/// String's own encoding. One the Encoding Standard has no label for
/// (UTF-32, CP51932, ...) is transcoded by Ruby, as before, and declared
/// UTF-8.
pub struct DocumentSource {
    source: HtmlSource,
    transport: Option<&'static crate::html::encoding::Encoding>,
}

impl DocumentSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(
        s: RString,
        given: Option<&'static crate::html::encoding::Encoding>,
    ) -> Result<DocumentSource, Error> {
        use crate::html::encoding::{transport_for, StringEncoding, Transport};
        let enc = Encoding::of(s);
        let transport = if enc.is_ascii8bit() {
            transport_for(given, StringEncoding::Binary)
        } else if enc.is_utf8_compatible() {
            transport_for(given, StringEncoding::Utf8)
        } else {
            /* Only an encoding of any other kind is asked its name. */
            let rb =
                magnus::encoding::RbEncoding::from(magnus::encoding::EncodingCapable::enc_get(s));
            transport_for(given, StringEncoding::Named(rb.name().as_bytes()))
        };
        let transport = match transport {
            Transport::Unlabelled => {
                return Ok(DocumentSource {
                    source: HtmlSource::from_ruby(s)?,
                    transport: Some(crate::html::encoding::UTF_8),
                })
            }
            Transport::Sniff => None,
            Transport::Declared(e) => Some(e),
        };
        /* Ruby's own verdict counts only for bytes it holds as UTF-8. */
        let known_valid = transport == Some(crate::html::encoding::UTF_8)
            && enc.is_utf8_compatible()
            && ruby_str_known_valid_utf8(s);
        let bytes = ruby_string_bytes(s)?;
        Ok(DocumentSource {
            source: HtmlSource { bytes, known_valid },
            transport,
        })
    }

    /// The input the parse reads, borrowed from this copy.
    pub fn input(&self) -> crate::html::encoding::DocumentInput<'_> {
        crate::html::encoding::DocumentInput {
            bytes: self.source.bytes(),
            transport: self.transport,
            known_valid_utf8: self.source.known_valid(),
        }
    }
}
