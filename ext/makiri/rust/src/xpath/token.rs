//! An erased node handle, opaque to the engine.
//!
//! A node-set stores tokens, a custom-function call and its arguments carry
//! them, and the string-value and document-order caches are keyed by them. The
//! engine only copies, compares and hashes a token; it never dereferences one.
//! Reading one back as a node is the backend's job (`Dom::resolve_token`), and
//! the Ruby bridge mints a token for a handler's node only after checking that
//! node belongs to the document being walked.
//!
//! That split is what lets this type be safe: a token is an opaque word (the
//! backend's own handle, cast through a pointer), and every operation the engine
//! performs on it is a comparison or a copy.

#![forbid(unsafe_code)]

use core::ffi::c_void;

/// The word no token names: an absent context node.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token(*mut c_void);

impl Token {
    /// The token for `p`, a backend handle or null. Never dereferences `p`.
    #[inline]
    pub fn from_ptr(p: *mut c_void) -> Token {
        Token(p)
    }

    /// The backend handle back, for the backend that made it.
    #[inline]
    pub fn as_ptr(self) -> *mut c_void {
        self.0
    }

    /// The token naming no node.
    #[inline]
    pub const fn null() -> Token {
        Token(core::ptr::null_mut())
    }

    /// True for [`null`](Self::null): no node.
    #[inline]
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}
