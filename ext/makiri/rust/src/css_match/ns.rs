//! A selector's namespace prefixes: the bindings a query is given
//! ([`Namespaces`]), and what each simple selector's prefix resolves to
//! ([`NsTest`]), settled when the selector is compiled - so a prefix nothing
//! binds is refused before any node is matched, whatever the document holds
//! (Selectors 4 §6.1: a selector with an undeclared prefix is invalid).

use crate::compat::{self, Fix};
use crate::selectors::ast::{Selector, Simple};

use super::MatchFailure;

/// The prefixes a query's selector may use, each bound to a namespace URI,
/// and the default namespace a bare type selector is in. A selector has no
/// `@namespace` of its own: the bindings come from the caller (the second
/// argument of `Node#css`).
#[derive(Clone, Copy)]
pub struct Namespaces<'a> {
    bindings: &'a [(&'a [u8], &'a [u8])],
    default: Option<&'a [u8]>,
}

impl<'a> Namespaces<'a> {
    /// No bindings: every prefix is undeclared, and a bare type selector
    /// matches its name in any namespace - what a browser's
    /// `querySelectorAll` has.
    pub const NONE: Namespaces<'static> = Namespaces {
        bindings: &[],
        default: None,
    };

    /// `bindings`, `(prefix, uri)` with the last binding of a prefix
    /// winning, and the default namespace if one is given.
    pub fn new(bindings: &'a [(&'a [u8], &'a [u8])], default: Option<&'a [u8]>) -> Self {
        Namespaces { bindings, default }
    }

    /// The URI an [`NsTest::Uri`] stands for.
    #[inline]
    pub(super) fn uri(&self, test: NsTest) -> Option<&'a [u8]> {
        match test.0 {
            NsTest::DEFAULT => self.default,
            k => self
                .bindings
                .get(usize::from(k - NsTest::FIRST))
                .map(|&(_, uri)| uri),
        }
    }
}

/// The namespace a type, universal or attribute selector asks for: any
/// (`*|E`, a bare `E` with no default namespace, and every simple selector
/// that names nothing), none (`|E`, `[|a]`, a bare `[a]`, a prefix bound to
/// ""), or one of the query's [`Namespaces`], by its place there - two
/// bytes, so a compiled simple selector stays the size it was.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct NsTest(u16);

impl NsTest {
    pub(super) const ANY: NsTest = NsTest(0);
    pub(super) const NONE: NsTest = NsTest(1);
    const FIRST: u16 = 2;
    const DEFAULT: u16 = u16::MAX;

    /// The test for the URI `uri`, which is `ns`'s at `at`: the empty URI
    /// is no namespace.
    fn uri_at(uri: &[u8], at: u16) -> Self {
        if uri.is_empty() {
            NsTest::NONE
        } else {
            NsTest(at)
        }
    }

    /// Whether this asks for one namespace (not any, not none).
    #[inline]
    pub(super) fn is_uri(self) -> bool {
        self.0 >= NsTest::FIRST
    }

    /// What `sel`'s namespace prefix resolves to under `ns` (`simple` is
    /// `sel`'s, decoded). A prefix `ns` does not bind is
    /// [`MatchFailure::UndeclaredPrefix`].
    ///
    /// Run as Lexbor ran (M6 in docs/parser_differences.ja.md), every prefix
    /// is ignored: a type selector matches its name in any namespace, and an
    /// attribute selector looks its name up as written.
    pub(super) fn resolve(
        sel: Selector<'_>,
        simple: Simple<'_>,
        ns: &Namespaces<'_>,
    ) -> Result<Self, MatchFailure> {
        let attribute = match simple {
            Simple::Type | Simple::Universal => false,
            Simple::Attribute(_) => true,
            _ => return Ok(NsTest::ANY),
        };
        if !compat::fixed(Fix::M6) {
            return Ok(if attribute { NsTest::NONE } else { NsTest::ANY });
        }
        Ok(match sel.ns() {
            /* An attribute's bare name is in no namespace; the default
             * namespace is an element's only. */
            None if attribute => NsTest::NONE,
            None => match ns.default {
                Some(uri) => NsTest::uri_at(uri, NsTest::DEFAULT),
                None => NsTest::ANY,
            },
            /* The parser stores `[|a]` with the namespace `*`, and refuses
             * `[*|a]` (C27). */
            Some(b"*") if attribute => NsTest::NONE,
            Some(b"*") => NsTest::ANY,
            Some(b"") => NsTest::NONE,
            Some(prefix) => {
                /* The last binding of a prefix wins. */
                let (k, &(_, uri)) = ns
                    .bindings
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, (p, _))| *p == prefix)
                    .ok_or(MatchFailure::UndeclaredPrefix)?;
                let at = u16::try_from(k)
                    .ok()
                    .and_then(|k| k.checked_add(NsTest::FIRST))
                    .filter(|&at| at != NsTest::DEFAULT)
                    .ok_or(MatchFailure::TooComplex)?;
                NsTest::uri_at(uri, at)
            }
        })
    }
}
