//! Borrowed text views: a `(ptr, len)` over bytes someone else owns.
//!
//! Two types with one shape, kept apart because their contracts differ:
//!
//! * [`VerifiedText`] - valid UTF-8 with **no NUL**. The engine-input contract:
//!   an XPath expression, a CSS selector, a namespace prefix or URI, a variable
//!   name or value. Made by checking the bytes ([`VerifiedText::from_bytes`]) or
//!   at the Ruby boundary, where the bridge has already run the same check.
//! * [`BorrowedText`] - valid UTF-8, **NUL permitted**. DOM text (HTML text and
//!   attribute data may hold U+0000, like browsers) and the strings an XPath
//!   evaluation produces from it.
//!
//! A `VerifiedText` converts into a `BorrowedText`, since the contract only
//! weakens; there is no conversion back, so DOM-derived bytes cannot reach a
//! place that assumes no NUL without being checked.
//!
//! Neither is NUL-terminated in general, so both are consumed as `(ptr, len)`,
//! never as a C string. Neither carries a lifetime either: the owner - a Ruby
//! String, Lexbor's arena, a `Text` - must outlive every use, which is why
//! reading the bytes is `unsafe`. A null pointer is the "absent" sentinel (an
//! omitted prefix, say), distinct from a present empty string.

use core::ffi::c_char;

/// The accessors both views share. The contracts differ; the reading does not.
macro_rules! view_accessors {
    () => {
        pub(crate) const fn as_ptr(self) -> *const c_char {
            self.ptr
        }

        pub(crate) const fn len(self) -> usize {
            self.len
        }

        pub(crate) const fn is_absent(self) -> bool {
            self.ptr.is_null()
        }

        /// No content. An absent view is empty too, but stays distinguishable
        /// through `is_absent`.
        pub(crate) const fn is_empty(self) -> bool {
            self.is_absent() || self.len == 0
        }

        /// The bytes, or an empty slice when absent.
        ///
        /// # Safety
        /// The owner of the bytes must stay live, at the same address, for `'a`.
        pub(crate) unsafe fn as_bytes<'a>(self) -> &'a [u8] {
            if self.is_empty() {
                &[]
            } else {
                // SAFETY: non-null, `len` live bytes by the constructor's
                // contract, and the caller keeps the owner alive for `'a`.
                unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len) }
            }
        }
    };
}

/// Valid UTF-8, no NUL: the text an engine input must be.
#[derive(Clone, Copy)]
pub struct VerifiedText {
    ptr: *const c_char,
    len: usize,
}

// Its constructors are called from the CSS lowering and the Ruby glue, so the
// Ruby-free builds (Kani, the fuzz crate) see them unused.
#[cfg_attr(not(feature = "ruby"), allow(dead_code))]
impl VerifiedText {
    view_accessors!();

    /// A present, zero-length view, backed by a static empty C string.
    #[cfg(test)]
    pub(crate) const fn empty() -> Self {
        Self {
            ptr: c"".as_ptr(),
            len: 0,
        }
    }

    /// Check `bytes` against the contract and borrow them.
    ///
    /// The caller keeps `bytes` alive, at the same address, for as long as the
    /// view is used.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if crate::cutf8::text_verdict(bytes, false) != crate::cutf8::TextVerdict::Ok {
            return None;
        }
        Some(Self {
            ptr: bytes.as_ptr() as *const c_char,
            len: bytes.len(),
        })
    }

    /// Adopt a `(ptr, len)` the Ruby bridge has already checked.
    ///
    /// # Safety
    /// `ptr` must be null (with `len == 0`) or point to `len` live bytes of
    /// valid UTF-8 containing no NUL, which stay put while the view is used.
    pub(crate) const unsafe fn from_raw_parts(ptr: *const c_char, len: usize) -> Self {
        Self { ptr, len }
    }
}

/// Valid UTF-8, NUL permitted: DOM text and the engine's own strings.
#[derive(Clone, Copy)]
pub struct BorrowedText {
    ptr: *const c_char,
    len: usize,
}

// Built by the text index and read by the Ruby glue, so the Ruby-free builds
// see most of it unused - and no build reads its `as_bytes` outside the tests
// since the engine's own strings became `xpath::value::Text`.
#[allow(dead_code)]
impl BorrowedText {
    view_accessors!();

    /// Borrow bytes from Lexbor's arena or an engine-owned slot.
    ///
    /// # Safety
    /// `ptr` must be null (with `len == 0`) or point to `len` live bytes of
    /// valid UTF-8, which stay put while the view is used. NUL is allowed.
    pub(crate) const unsafe fn from_raw_parts(ptr: *const c_char, len: usize) -> Self {
        Self { ptr, len }
    }
}

impl From<VerifiedText> for BorrowedText {
    fn from(t: VerifiedText) -> Self {
        Self {
            ptr: t.ptr,
            len: t.len,
        }
    }
}
