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
//! monomorphic at the Ruby boundary. Both kinds a token can name are arena
//! handles, which their resolvers read through a checked lookup, so any word
//! is safe to make a token of: a forged or stale one resolves to no node,
//! never to undefined behaviour. The kind keeps a token from the other
//! backend's resolver. (It also tagged a Lexbor node pointer, made by an
//! `unsafe` constructor and read back by a dereferencing resolver, until the
//! Lexbor DOM build was retired.)

#![forbid(unsafe_code)]

/// Which backend a token belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// No node: the empty-slot marker of token-keyed tables. Never resolved.
    Null,
    /// A Makiri XML arena handle.
    Xml,
    /// A node id of the HTML arena DOM (`crate::html::dom`).
    Arena,
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
    pub const fn xml(word: usize) -> Token {
        Token {
            kind: Kind::Xml,
            word,
        }
    }

    /// A token for a node id of the HTML arena DOM.
    ///
    /// Safe for the reason [`Token::xml`] is: the arena's resolver reads an
    /// id through a bounds-checked lookup, so no value can cause undefined
    /// behaviour - and the kind keeps it from the XML resolver.
    #[inline]
    pub const fn arena(word: usize) -> Token {
        Token {
            kind: Kind::Arena,
            word,
        }
    }

    /// The handle back, for the backend that made it.
    #[inline]
    pub const fn word(self) -> usize {
        self.word
    }
}

/// A token keys the engine's per-evaluate tables (the string-value cache, the
/// document-order index). A real token's word is never 0 - an arena word packs
/// the node index under a nonzero document stamp - but the table still needs a
/// value no real token equals, so
/// the empty slot carries a kind (`Null`) that no real token has.
impl crate::ptr_table::TableKey for Token {
    const EMPTY: Token = Token::null();
    #[inline]
    fn table_hash(self) -> u64 {
        crate::ptr_table::mix64(self.word as u64)
    }
}
