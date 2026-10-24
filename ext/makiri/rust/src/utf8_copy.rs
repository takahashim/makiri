//! `Utf8Copy`: an owned copy of some bytes, made only once the copy is
//! checked to be UTF-8 - so it can be read as a `&str` with no further check.
//!
//! The bridge's checked views of a Ruby String (`RubyText`, `RubyData`) are
//! this copy, which is why it is Ruby-free: the one claim a `&str` rests on -
//! these bytes are UTF-8 - is made here, by std's validator over the copy
//! itself, and depends on nothing a Ruby String says about itself (its cached
//! coderange, which another C extension or a Ruby bug could leave wrong).
//! The fields are private, so nothing outside this module can change the
//! bytes once they are checked.

#![allow(unsafe_code)]

use crate::cbuf::OwnedBuf;

/// Up to this many bytes a copy lives inline, on the stack: a selector, an
/// expression or a name allocates nothing.
pub const INLINE: usize = 64;

enum Bytes {
    Inline { len: u8, buf: [u8; INLINE] },
    Heap(OwnedBuf),
}

/// Why no [`Utf8Copy`] was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError {
    /// The bytes are not UTF-8.
    NotUtf8,
    /// A copy past [`INLINE`] bytes could not be allocated.
    Oom,
}

/// Bytes that are UTF-8, owned.
pub struct Utf8Copy(Bytes);

impl Utf8Copy {
    /// A copy of `src`, checked to be UTF-8.
    pub fn new(src: &[u8]) -> Result<Utf8Copy, CopyError> {
        let bytes = match (src.len(), src) {
            (n, _) if n <= INLINE => {
                let mut buf = [0u8; INLINE];
                if let Some(dst) = buf.get_mut(..n) {
                    dst.copy_from_slice(src);
                }
                Bytes::Inline { len: n as u8, buf }
            }
            _ => Bytes::Heap(OwnedBuf::copy_from(src).ok_or(CopyError::Oom)?),
        };
        let copy = Utf8Copy(bytes);
        /* The check is of the copy, not of `src`: what is read back later is
         * exactly what was checked. */
        match core::str::from_utf8(copy.as_bytes()) {
            Ok(_) => Ok(copy),
            Err(_) => Err(CopyError::NotUtf8),
        }
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Bytes::Inline { len, buf } => buf.get(..usize::from(*len)).unwrap_or(&[]),
            Bytes::Heap(b) => b.as_slice(),
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        // SAFETY: a `Utf8Copy` is made only by `new`, which returns it only
        // once `from_utf8` accepted exactly these bytes, and nothing changes
        // them after: the fields are private to this module, and no method
        // here writes them.
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_text_reads_back_inline_and_on_the_heap() {
        for s in ["", "a", "é", &"x".repeat(INLINE), &"é".repeat(INLINE)] {
            assert_eq!(Utf8Copy::new(s.as_bytes()).unwrap().as_str(), s);
        }
    }

    #[test]
    fn invalid_bytes_are_refused_inline_and_on_the_heap() {
        assert_eq!(Utf8Copy::new(b"\xff").err(), Some(CopyError::NotUtf8));
        let mut long = [b'a'; INLINE * 2 + 1];
        long[INLINE * 2] = 0xc3;
        assert_eq!(Utf8Copy::new(&long).err(), Some(CopyError::NotUtf8));
        /* A cut multi-byte character at the inline limit. */
        let mut edge = [b'a'; INLINE];
        edge[INLINE - 1] = 0xc3;
        assert_eq!(Utf8Copy::new(&edge).err(), Some(CopyError::NotUtf8));
    }
}
