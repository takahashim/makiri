//! Borrowed text views: a `(ptr, len)` over bytes someone else owns.
//!
//! [`VerifiedText`] - valid UTF-8 with **no NUL**. The engine-input contract:
//! an XPath expression, a CSS selector, a namespace prefix or URI, a variable
//! name or value. Made by checking the bytes ([`VerifiedText::from_bytes`]) or
//! at the Ruby boundary, where the bridge has already run the same check.
//!
//! It is not NUL-terminated in general, so it is consumed as `(ptr, len)`,
//! never as a C string. A null pointer is the "absent" sentinel (an omitted
//! prefix, say), distinct from a present empty string.
//!
//! [`VerifiedText`] carries a lifetime: its owner must outlive `'a`, so an
//! engine call cannot hold the token past the borrow `as_verified` took. Its
//! `as_bytes` is then safe - the borrow was established with the token, and the
//! engine never runs Ruby that could move the bytes.

#![allow(unsafe_code)]

use core::ffi::c_char;

/// The view's shared accessors.
macro_rules! view_accessors {
    () => {
        /// The pointer. The tests pin it to the owner's own; code reads
        /// `as_bytes`.
        #[cfg_attr(not(test), allow(dead_code))]
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
    };
}

/// Valid UTF-8, no NUL: the text an engine input must be.
///
/// The lifetime is the borrow of the bytes' owner, taken by [`from_bytes`] or
/// asserted by [`from_raw_parts`]; it is what makes [`as_bytes`] safe and stops
/// a token outliving the bytes it names.
#[derive(Clone, Copy)]
pub struct VerifiedText<'a> {
    ptr: *const c_char,
    len: usize,
    _borrow: core::marker::PhantomData<&'a [u8]>,
}

// Its constructors are called from the CSS lowering and the Ruby glue, so the
// Ruby-free builds (Kani, the fuzz crate) see them unused.
#[cfg_attr(not(feature = "ruby"), allow(dead_code))]
impl<'a> VerifiedText<'a> {
    view_accessors!();

    /// The bytes, or an empty slice when absent.
    ///
    /// Safe: the bytes live for `'a` by the constructor's contract.
    pub(crate) fn as_bytes(self) -> &'a [u8] {
        if self.is_empty() {
            &[]
        } else {
            // SAFETY: the bytes live for `'a` by the constructor's contract.
            unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len) }
        }
    }

    /// A present, zero-length view, backed by a static empty C string.
    #[cfg(test)]
    pub(crate) const fn empty() -> Self {
        Self {
            ptr: c"".as_ptr(),
            len: 0,
            _borrow: core::marker::PhantomData,
        }
    }

    /// Check `bytes` against the contract and borrow them.
    pub fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        if crate::cutf8::text_verdict(bytes, false) != crate::cutf8::TextVerdict::Ok {
            return None;
        }
        Some(Self {
            ptr: bytes.as_ptr() as *const c_char,
            len: bytes.len(),
            _borrow: core::marker::PhantomData,
        })
    }

    /// Adopt a `(ptr, len)` the Ruby bridge has already checked.
    ///
    /// # Safety
    /// `ptr` must be null (with `len == 0`) or point to `len` live bytes of
    /// valid UTF-8 containing no NUL, which stay put for `'a`.
    pub(crate) const unsafe fn from_raw_parts(ptr: *const c_char, len: usize) -> Self {
        Self {
            ptr,
            len,
            _borrow: core::marker::PhantomData,
        }
    }
}
