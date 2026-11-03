//! A Ruby String as parser input: [`SourceBytes`] (its bytes, copied),
//! [`HtmlSource`] (a fragment parse's, transcoded to UTF-8) and
//! [`DocumentSource`] (a document parse's, with the transport layer's
//! encoding).

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, RString};
use crate::bridge::ruby::makiri_error;

/// The shared owned buffer.
use crate::ceiling_buf::OwnedBuf;

use super::*;

/// A Ruby String's bytes, copied out, and whether they are already known
/// to be valid UTF-8 - never a scan, only what Ruby (or a transcode) has
/// established - so a parser can skip its sanitisation. The bytes are what
/// they are: whether they are read as the String's encoding is the maker's
/// ([`HtmlSource`] transcodes, [`DocumentSource`] leaves it to the sniff).
/// The copy is what a parse needs anyway, to read the source with the GVL
/// released, and the engine below never sees a `VALUE`.
pub struct SourceBytes {
    bytes: OwnedBuf,
    known_valid: bool,
}

impl SourceBytes {
    /// Whether the bytes are valid UTF-8 already.
    pub fn known_valid(&self) -> bool {
        self.known_valid
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

/// A Ruby String as a FRAGMENT parse's input, under the text-input contract:
/// its encoding honoured (`nupur::string::to_utf8`, replacing what it cannot
/// convert), so its bytes are UTF-8.
pub struct HtmlSource(SourceBytes);

impl HtmlSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(ruby: &crate::bridge::rb::Ruby, s: RString) -> Result<HtmlSource, Error> {
        Self::read_as(ruby, s, None)
    }

    /// [`from_ruby`](Self::from_ruby), but its bytes read as `read_as`'s
    /// encoding whatever the String is tagged with (`None`: as its own).
    /// Every invalid or unmappable byte is replaced; a String whose encoding
    /// Ruby cannot convert at all raises Ruby's own `EncodingError`, as it
    /// was.
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
                Transcode::AsIs(src) => (src, src.cached_utf8()),
                /* The transcode replaced every invalid or unmappable byte, so
                 * its result is valid UTF-8 whatever its coderange says. */
                Transcode::Transcoded(src) => (src, true),
                Transcode::Fault(e) => return Err(e),
            };
        let bytes = ruby_string_bytes(src)?;
        Ok(HtmlSource(SourceBytes { bytes, known_valid }))
    }

    /// Whether the bytes are valid UTF-8 already ([`SourceBytes`]).
    pub fn known_valid(&self) -> bool {
        self.0.known_valid()
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// A Ruby String as a DOCUMENT parse's input (`html::encoding`): its bytes
/// as they are, and the transport layer's encoding they come under - the
/// given encoding's (`encoding:`), else the String's own, both read by
/// `html::encoding::transport_for`. One the Encoding Standard has no label
/// for (UTF-32, CP51932, ...) is transcoded by Ruby and declared UTF-8.
pub struct DocumentSource {
    source: SourceBytes,
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
                    source: HtmlSource::read_as(ruby, s, Some(enc))?.0,
                    transport: Some(UTF_8),
                })
            }
            Transport::Sniff => Ok(DocumentSource {
                source: SourceBytes {
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
        /* Ruby's own verdict (`RString::cached_utf8`, which counts it only for bytes
         * it holds as UTF-8) is of use only where the bytes are read as
         * UTF-8. */
        let known_valid = e == crate::html::encoding::UTF_8 && s.cached_utf8();
        Ok(DocumentSource {
            source: SourceBytes {
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
