//! [`VerifiedText`]: a borrowed engine input - an XPath expression, a CSS
//! selector, a namespace prefix or URI, a variable name or value.
//!
//! The one contract is **valid UTF-8 with no NUL**. The UTF-8 half is the type
//! (`&str`); the NUL half is [`crate::cutf8::text_verdict`], the one check this
//! type and the Ruby bridge's `text_check` both run. Made by checking the bytes
//! ([`VerifiedText::from_bytes`], [`VerifiedText::new`]) or at the Ruby boundary,
//! where the bridge has already run the same check
//! ([`VerifiedText::from_checked`]).
//!
//! No NUL is a LOGICAL contract, not a memory-safety one: every consumer reads
//! the text as a slice - the XPath lexer, the CSS lowering, and Lexbor's
//! selector parser, which is handed `(ptr, len)` - and none as a C string. It
//! is what the engine promises its callers about names (a NUL cannot be
//! serialized into markup, nor written in an XPath or CSS literal), so it is
//! checked once, here, rather than wherever a name is used.
//!
//! DOM text, which may hold U+0000 like browsers, is not a `VerifiedText`, and
//! there is no conversion into one that skips the check.

#![forbid(unsafe_code)]

use core::ops::Deref;

use crate::cutf8::{text_verdict, TextVerdict};

/// Valid UTF-8 (by type) with no NUL (by construction): the text an engine
/// input must be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VerifiedText<'a>(&'a str);

// Its constructors are called from the CSS lowering and the Ruby glue, so the
// Ruby-free builds (Kani, the fuzz crate) see some of them unused.
#[cfg_attr(not(feature = "ruby"), allow(dead_code))]
impl<'a> VerifiedText<'a> {
    /// `s`, unless it holds a NUL.
    ///
    /// The NUL half of the contract is [`cutf8::text_verdict`], the one check
    /// the bridge's [`text_check`](crate::bridge::string::text_check) also runs;
    /// `true` because `s` is already UTF-8 by type.
    pub fn new(s: &'a str) -> Option<Self> {
        matches!(text_verdict(s.as_bytes(), true), TextVerdict::Ok).then_some(Self(s))
    }

    /// Check `bytes` against the contract and borrow them.
    pub fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        Self::new(core::str::from_utf8(bytes).ok()?)
    }

    /// Adopt text the Ruby bridge has already checked against the same
    /// contract (`bridge::string::text_check`).
    pub(crate) fn from_checked(s: &'a str) -> Self {
        debug_assert!(!s.as_bytes().contains(&0), "a checked text holds a NUL");
        Self(s)
    }

    pub fn as_str(self) -> &'a str {
        self.0
    }
}

impl Deref for VerifiedText<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        self.0
    }
}
