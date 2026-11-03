//! Ruby encodings: a String's own, and transcoding one to UTF-8.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::Error;
pub use crate::nupur::encoding::Encoding;

/* ---- encoding ---- */

/* Reading a String as UTF-8 is nupur's (`nupur::encoding`). */
pub use crate::nupur::encoding::to_encoding;

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

/// Whether a String's own tag claims no encoding for the document it holds:
/// ASCII-8BIT, and US-ASCII. A document parse - HTML's and XML's alike -
/// reads such a String by what its bytes declare (a BOM, a `<meta>` or
/// `encoding=`), and as UTF-8 when they declare nothing.
///
/// US-ASCII is in that group because it is what a String gets with no claim
/// behind it - `File.read` under `LANG=C` - not a promise about the bytes: a
/// UTF-16 file with its BOM, or a Latin-1 one declaring ISO-8859-1, read that
/// way was validated as UTF-8 and refused, where the same bytes tagged
/// ASCII-8BIT decoded. (A US-ASCII a DOCUMENT declares is a claim:
/// `xml::decode` holds the bytes to it.)
pub(in crate::bridge) fn claims_no_encoding(tag: Encoding) -> bool {
    tag.is_ascii8bit() || tag.is_usascii()
}

/// How a document parse reads `enc` (`html::encoding::transport_for`): a
/// String that claims no encoding ([`claims_no_encoding`]) as binary. Only
/// an encoding of a kind other than binary, UTF-8 and US-ASCII is asked its
/// name.
pub(in crate::bridge) fn encoding_kind(
    enc: Encoding,
) -> crate::html::encoding::StringEncoding<'static> {
    use crate::html::encoding::StringEncoding;
    if claims_no_encoding(enc) {
        StringEncoding::Binary
    } else if enc.is_utf8_or_ascii() {
        StringEncoding::Utf8
    } else {
        StringEncoding::Named(enc.name())
    }
}
