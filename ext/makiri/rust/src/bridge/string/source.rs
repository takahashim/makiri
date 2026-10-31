//! A Ruby String as parser input: [`HtmlSource`] (fragments, and the bytes
//! a document parse copies) and [`DocumentSource`] (a document parse, with
//! the transport layer's encoding).

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, RString};
use crate::bridge::ruby::makiri_error;

/// The shared owned buffer.
use crate::ceiling_buf::OwnedBuf;

use super::*;

/// A Ruby String as HTML parser input, under the text-input contract: its
/// encoding honoured (`nupur::string::to_utf8`, replacing what it cannot
/// convert), its bytes copied out, and
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
        Self::read_as(ruby, s, None)
    }

    /// [`from_ruby`](Self::from_ruby), its bytes read as `read_as`'s
    /// whatever the String says (`None`: as its own). Every invalid or
    /// unmappable byte is replaced; a String whose encoding Ruby cannot
    /// convert at all raises Ruby's own `EncodingError`, as it was.
    pub fn read_as(
        ruby: &crate::bridge::rb::Ruby,
        s: RString,
        read_as: Option<Encoding>,
    ) -> Result<HtmlSource, Error> {
        use crate::nupur::encoding::Unconvertible;
        use crate::nupur::string::Transcode;
        let (src, known_valid) =
            match crate::nupur::string::to_utf8(ruby, s, read_as, Unconvertible::Replace)? {
                /* Ruby's cached verdict on the bytes, only a hint. */
                Transcode::AsIs(src) => (src, src.known_utf8()),
                /* The transcode replaced every invalid or unmappable byte, so
                 * its result is valid UTF-8 whatever its coderange says. */
                Transcode::Transcoded(src) => (src, true),
                Transcode::Fault(e) => return Err(e),
            };
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
            Some(GivenEncoding::Label(e)) => return Self::declared(s, e),
            Some(GivenEncoding::Ruby(e)) => e,
            None => own,
        };
        match transport_for(super::encoding_kind(enc)) {
            Transport::Unlabelled => {
                /* The bytes are read as `enc`'s, whatever the String says. */
                Ok(DocumentSource {
                    source: HtmlSource::read_as(ruby, s, Some(enc))?,
                    transport: Some(UTF_8),
                })
            }
            Transport::Sniff => Ok(DocumentSource {
                source: HtmlSource {
                    bytes: ruby_string_bytes(s)?,
                    known_valid: false,
                },
                transport: None,
            }),
            Transport::Declared(e) => Self::declared(s, e),
        }
    }

    /// `s`'s bytes, declared to be in `e`.
    fn declared(
        s: RString,
        e: &'static crate::html::encoding::Encoding,
    ) -> Result<DocumentSource, Error> {
        /* Ruby's own verdict (`RString::known_utf8`, which counts it only for bytes
         * it holds as UTF-8) is of use only where the bytes are read as
         * UTF-8. */
        let known_valid = e == crate::html::encoding::UTF_8 && s.known_utf8();
        Ok(DocumentSource {
            source: HtmlSource {
                bytes: ruby_string_bytes(s)?,
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

/* ---- a String's bytes, copied out ---- */

/// The first `N` bytes of `s` (all of them when it is shorter), copied, with
/// how many there are: what a sniff of a document's start reads, owned.
pub(in crate::bridge) fn prefix<const N: usize>(s: RString) -> ([u8; N], usize) {
    let mut out = [0u8; N];
    let n = s.copy_to(0, &mut out);
    (out, n)
}

/// A live Ruby String's bytes from `off` (none past its end), copied into an
/// owned buffer reserved for exactly them.
pub fn ruby_string_bytes_from(s: RString, off: usize) -> Result<OwnedBuf, Error> {
    let n = s.len().saturating_sub(off);
    /* Nothing runs between the reservation and the copy that could change
     * the String; `append_to` checks the room again, and refuses rather than
     * grows. */
    OwnedBuf::reserved(n, |v| s.append_to(off, v).is_ok())
        .ok_or_else(|| makiri_error("out of memory reading a Ruby string"))
}

/// A live Ruby String's raw bytes, copied into an owned buffer.
pub fn ruby_string_bytes(s: RString) -> Result<OwnedBuf, Error> {
    ruby_string_bytes_from(s, 0)
}
