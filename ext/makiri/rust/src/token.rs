//! An erased, kind-tagged node handle: what a node-set stores and a
//! custom-function call and its arguments carry.
//!
//! The engine only copies, compares and hashes a token; it never dereferences
//! one. Reading one back as a node is the backend's job (`Dom::resolve_token`),
//! and a token is only ever made by a backend's `Dom::token` (from a node that
//! backend just lent) or, for a Ruby handler's node, by the bridge after it has
//! checked the node's document.
//!
//! # Why the kind lives in the type
//!
//! The engine erases a token to this one type so `Val`/`NodeSet` stay
//! monomorphic at the Ruby boundary. The cost is that a *safe* constructor plus
//! a dereferencing resolver would let safe code forge a token and read it back
//! as a node - undefined behaviour without an `unsafe` in sight. The tag closes
//! that: an HTML token can only be made by [`Token::html`], which is `unsafe`
//! (the caller proves the pointer names a live node), while an XML token is
//! [`Token::xml`], safe because the XML resolver checks every handle it reads.
//! A resolver rejects a token of the wrong kind, so a forged XML token can never
//! reach the pointer-dereferencing HTML resolver.

#![allow(unsafe_code)]

use core::ffi::c_void;

/// Which backend a token belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// No node: the empty-slot marker of token-keyed tables. Never resolved.
    Null,
    /// A Lexbor node pointer.
    Html,
    /// A Makiri XML arena handle.
    Xml,
}

/// An opaque node handle, tagged with its backend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Token {
    kind: Kind,
    word: usize,
}

impl Token {
    /// The token naming no node - the empty slot of a token-keyed table (see
    /// the `TableKey` impl below). An absent context node is `None`, not this,
    /// so this stays crate-internal: only the table ever makes one.
    #[inline]
    pub(crate) const fn null() -> Token {
        Token {
            kind: Kind::Null,
            word: 0,
        }
    }

    /// Which backend made this token.
    #[inline]
    pub const fn kind(self) -> Kind {
        self.kind
    }

    /// A token for an XML arena handle.
    ///
    /// Safe: the XML resolver reads a handle through `Document::try_node`, which
    /// rejects a stale or foreign one, so no handle value can cause undefined
    /// behaviour.
    #[inline]
    pub(crate) const fn xml(word: usize) -> Token {
        Token {
            kind: Kind::Xml,
            word,
        }
    }

    /// The XML handle back, for the XML backend.
    #[inline]
    pub const fn word(self) -> usize {
        self.word
    }

    /// A token for an HTML node pointer.
    ///
    /// # Safety
    /// `ptr` must name a live node of the document the token will be resolved
    /// against.
    #[inline]
    pub unsafe fn html(ptr: *mut c_void) -> Token {
        Token {
            kind: Kind::Html,
            word: ptr as usize,
        }
    }

    /// The HTML node pointer back, for the HTML backend.
    #[inline]
    pub fn as_ptr(self) -> *mut c_void {
        self.word as *mut c_void
    }
}

/// A token keys the engine's per-evaluate tables (the string-value cache, the
/// document-order index). Its word alone cannot mark an empty slot - an XML
/// token is an arena index, and 0 is one - so the empty slot is the null token.
impl crate::ptr_table::TableKey for Token {
    const EMPTY: Token = Token::null();
    #[inline]
    fn table_hash(self) -> u64 {
        crate::ptr_table::mix64(self.word as u64)
    }
}
