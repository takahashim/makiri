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
//!
//! It depends on std alone, so the extension's Ruby-free test build includes
//! this same file (`#[path]`), where Miri runs its tests without a Ruby
//! (`rake miri`). A copy's heap allocation is fallible, and the extension's
//! OOM sweep can fail it ([`set_alloc_injector`]).

#![allow(unsafe_code)]

/// The function that answers whether the next heap copy fails, registered
/// by an extension's OOM sweep ([`set_alloc_injector`]).
#[cfg(feature = "alloc-inject")]
static ALLOC_INJECTOR: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Register the OOM sweep's answer, once (nupur's `inject::install`, which
/// registers the raise half with it); a second registration is ignored.
#[cfg(feature = "alloc-inject")]
pub(crate) fn set_alloc_injector(fails: fn() -> bool) {
    let _ = ALLOC_INJECTOR.set(fails);
}

/// Room for exactly `len` more bytes in the empty `v`, or `false`: a heap
/// copy's one allocation, which the OOM sweep can fail.
fn reserve_exact(v: &mut Vec<u8>, len: usize) -> bool {
    #[cfg(feature = "alloc-inject")]
    if ALLOC_INJECTOR.get().is_some_and(|fails| fails()) {
        return false;
    }
    #[allow(
        clippy::disallowed_methods,
        reason = "std's fallible reserve: this module depends on std alone, and the sweep's hook is above"
    )]
    v.try_reserve_exact(len).is_ok()
}

/// Up to this many bytes a copy lives inline, on the stack: a selector, an
/// expression or a name allocates nothing.
pub const INLINE: usize = 64;

enum Bytes {
    /* A `usize` length, so no conversion stands between `INLINE` and it. */
    Inline { len: usize, buf: [u8; INLINE] },
    Heap(Vec<u8>),
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

/// Why [`Utf8Copy::filled`] made no copy: [`CopyError`]'s reasons, or the
/// caller's own refusal of the bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillError<P> {
    /// The copy could not be made ([`CopyError`]).
    Copy(CopyError),
    /// `refuse` turned the bytes down.
    Refused(P),
}

/// What a [`Utf8Copy`] is copied from: a thing of known length that copies
/// itself out - into a buffer as given, or appended to a `Vec` whose room is
/// already reserved for it, which it must not grow. A byte slice is one; a
/// Ruby String is another (the bridge's, over `RString::copy_to` /
/// `append_to`), which hands no slice of itself out.
pub trait Source {
    /// How many bytes there are.
    fn byte_len(&self) -> usize;
    /// Copy them into `dst`, as many as fit; how many.
    fn copy_into(&self, dst: &mut [u8]) -> usize;
    /// Append them to `v`, whose spare room holds them; `false`, with `v`
    /// unchanged, when it does not.
    fn append_into(&self, v: &mut Vec<u8>) -> bool;
}

impl Source for &[u8] {
    fn byte_len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn copy_into(&self, dst: &mut [u8]) -> usize {
        let n = <[u8]>::len(self).min(dst.len());
        if let (Some(d), Some(s)) = (dst.get_mut(..n), self.get(..n)) {
            d.copy_from_slice(s);
        }
        n
    }

    fn append_into(&self, v: &mut Vec<u8>) -> bool {
        if v.capacity() - v.len() < <[u8]>::len(self) {
            return false;
        }
        #[allow(
            clippy::disallowed_methods,
            reason = "the room was checked above, so it cannot grow"
        )]
        v.extend_from_slice(self);
        true
    }
}

impl Utf8Copy {
    /// A copy of `src`, checked to be UTF-8.
    pub fn new(src: &[u8]) -> Result<Utf8Copy, CopyError> {
        Utf8Copy::from_source(&src, |_| None::<core::convert::Infallible>).map_err(|e| match e {
            FillError::Copy(e) => e,
            FillError::Refused(never) => match never {},
        })
    }

    /// A copy of `src`, put to `refuse` and then checked to be UTF-8.
    ///
    /// Up to [`INLINE`] bytes the copy is on the stack (`copy_into`); past
    /// that it is a heap buffer reserved for exactly them and appended to
    /// (`append_into`), so it is written once, not zeroed first.
    ///
    /// `refuse` sees the copy before the UTF-8 check, so a caller's own rule
    /// (a NUL, a length) is reported ahead of invalid UTF-8, and both look at
    /// the same bytes that are read back later.
    pub fn from_source<P>(
        src: &impl Source,
        refuse: impl FnOnce(&[u8]) -> Option<P>,
    ) -> Result<Utf8Copy, FillError<P>> {
        let len = src.byte_len();
        let bytes = if len <= INLINE {
            let mut buf = [0u8; INLINE];
            let n = src.copy_into(buf.get_mut(..len).unwrap_or_default());
            /* A short copy is a failed one, as on the heap path below: a
             * truncated text must never pass for the whole. */
            if n != len {
                return Err(FillError::Copy(CopyError::Oom));
            }
            Bytes::Inline { len: n, buf }
        } else {
            let mut v = Vec::new();
            if !reserve_exact(&mut v, len) || !src.append_into(&mut v) {
                return Err(FillError::Copy(CopyError::Oom));
            }
            Bytes::Heap(v)
        };
        let copy = Utf8Copy(bytes);
        if let Some(p) = refuse(copy.as_bytes()) {
            return Err(FillError::Refused(p));
        }
        /* The check is of the copy, not of its source: what is read back
         * later is exactly what was checked. */
        match core::str::from_utf8(copy.as_bytes()) {
            Ok(_) => Ok(copy),
            Err(_) => Err(FillError::Copy(CopyError::NotUtf8)),
        }
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Bytes::Inline { len, buf } => buf.get(..*len).unwrap_or(&[]),
            Bytes::Heap(b) => b,
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        // SAFETY: a `Utf8Copy` is made only by `from_source`, which returns it only
        // once `from_utf8` accepted exactly these bytes, and nothing changes
        // them after: the fields are private to this module, and no method
        // here writes them.
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
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

    #[test]
    fn a_refusal_comes_before_the_utf8_check_and_sees_the_copy() {
        for n in [3, INLINE * 2] {
            let mut src = vec![b'a'; n];
            src[0] = 0;
            src[n - 1] = 0xff;
            let got = Utf8Copy::from_source(&src.as_slice(), |b| b.contains(&0).then_some("nul"));
            assert_eq!(got.err(), Some(FillError::Refused("nul")));
        }
    }

    /// A source whose room check fails is out of memory, not a short copy.
    #[test]
    fn a_source_that_will_not_append_is_refused() {
        struct Short;
        impl Source for Short {
            fn byte_len(&self) -> usize {
                INLINE * 2
            }
            fn copy_into(&self, _: &mut [u8]) -> usize {
                0
            }
            fn append_into(&self, _: &mut Vec<u8>) -> bool {
                false
            }
        }
        let got = Utf8Copy::from_source(&Short, |_| None::<()>);
        assert_eq!(got.err(), Some(FillError::Copy(CopyError::Oom)));
    }

    /// The same, inline: a source that copies fewer bytes than it reported is
    /// refused, not truncated.
    #[test]
    fn a_source_that_copies_short_inline_is_refused() {
        struct Short;
        impl Source for Short {
            fn byte_len(&self) -> usize {
                INLINE
            }
            fn copy_into(&self, _: &mut [u8]) -> usize {
                INLINE - 1
            }
            fn append_into(&self, _: &mut Vec<u8>) -> bool {
                false
            }
        }
        let got = Utf8Copy::from_source(&Short, |_| None::<()>);
        assert_eq!(got.err(), Some(FillError::Copy(CopyError::Oom)));
    }
}
