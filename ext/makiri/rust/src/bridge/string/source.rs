//! A Ruby String as parser input: [`HtmlSource`] (fragments, and the bytes
//! a document parse copies) and [`DocumentSource`] (a document parse, with
//! the transport layer's encoding).

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, RString};

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
    pub fn from_ruby(ruby: &crate::bridge::rb::Ruby, s: RString) -> Result<HtmlSource, Error> {
        let src = ruby_to_utf8_value(ruby, s)?;
        /* A transcode replaced every invalid or unmappable byte, so its result
         * is valid UTF-8 whatever its coderange says. */
        let transcoded = !crate::bridge::ruby::same_value(src, s);
        let known_valid = transcoded || ruby_str_known_valid_utf8(ruby, src);
        let bytes = ruby_string_bytes(ruby, src)?;
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

/// The encoding `encoding:` names, read the way the String's own encoding
/// is read: by Ruby's name for it first, so `"US-ASCII"`, `"BINARY"` and
/// `Encoding::UTF_32LE` mean what a String tagged with them means, and an
/// Encoding Standard label Ruby does not know (`"latin1"`, `"unicode"`)
/// second.
#[derive(Clone, Copy)]
pub enum GivenEncoding {
    /// An encoding Ruby knows.
    Ruby(Encoding),
    /// An Encoding Standard label Ruby does not know.
    Label(&'static crate::html::encoding::Encoding),
}

impl GivenEncoding {
    /// The encoding `name` names, or None for a name neither Ruby nor the
    /// Encoding Standard knows. May autoload a Ruby encoding - see
    /// [`Encoding::find`], whose `Err` (not an unknown name) is passed on.
    pub fn resolve(ruby: &Ruby, name: &[u8]) -> Result<Option<GivenEncoding>, Error> {
        Ok(Encoding::find(ruby, name)?
            .map(GivenEncoding::Ruby)
            .or_else(|| crate::html::encoding::for_label(name).map(GivenEncoding::Label)))
    }
}

/// A Ruby String as a DOCUMENT parse's input (`html::encoding`): its bytes
/// as they are, and the transport layer's encoding they come under - the
/// given encoding's (`encoding:`), else the String's own, both read by
/// `html::encoding::transport_for`. One the Encoding Standard has no label
/// for (UTF-32, CP51932, ...) is transcoded by Ruby and declared UTF-8.
pub struct DocumentSource {
    source: HtmlSource,
    transport: Option<&'static crate::html::encoding::Encoding>,
}

impl DocumentSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(
        ruby: &crate::bridge::rb::Ruby,
        s: RString,
        given: Option<GivenEncoding>,
    ) -> Result<DocumentSource, Error> {
        use crate::html::encoding::{transport_for, Transport, UTF_8};
        let own = Encoding::of(s);
        let enc = match given {
            Some(GivenEncoding::Label(e)) => return Self::declared(ruby, s, own, e),
            Some(GivenEncoding::Ruby(e)) => e,
            None => own,
        };
        match transport_for(super::encoding_kind(ruby, enc)) {
            Transport::Unlabelled => {
                /* The bytes are read as `enc`'s, whatever the String says. */
                let s = if enc == own { s } else { enc.relabel(s)? };
                Ok(DocumentSource {
                    source: HtmlSource::from_ruby(ruby, s)?,
                    transport: Some(UTF_8),
                })
            }
            Transport::Sniff => Ok(DocumentSource {
                source: HtmlSource {
                    bytes: ruby_string_bytes(ruby, s)?,
                    known_valid: false,
                },
                transport: None,
            }),
            Transport::Declared(e) => Self::declared(ruby, s, own, e),
        }
    }

    /// `s`'s bytes, declared to be in `e`.
    fn declared(
        ruby: &crate::bridge::rb::Ruby,
        s: RString,
        own: Encoding,
        e: &'static crate::html::encoding::Encoding,
    ) -> Result<DocumentSource, Error> {
        /* Ruby's own verdict counts only for bytes it holds as UTF-8. */
        let known_valid = e == crate::html::encoding::UTF_8
            && own.is_utf8_compatible(ruby)
            && ruby_str_known_valid_utf8(ruby, s);
        Ok(DocumentSource {
            source: HtmlSource {
                bytes: ruby_string_bytes(ruby, s)?,
                known_valid,
            },
            transport: Some(e),
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
