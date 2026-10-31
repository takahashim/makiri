//! What both representations' `#css` / `#at_css` / `#matches?` share: the
//! argument list, the selector's check and compile, its namespace bindings,
//! and what each failure raises ([`CssQuery`]). A representation brings only
//! its node, and wraps what comes back.
//!
//! One wording, so `Makiri::HTML` and `Makiri::XML` answer a bad selector the
//! same way: `"<reason>: <selector>"`, where the reason is the engine's when it
//! has one ("undeclared namespace prefix") and a plain "invalid CSS selector"
//! otherwise.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, RHash, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::css_cache::{Kept, ParseError};
use crate::css_match::dom::CssNode;
use crate::css_match::{MatchFailure, Namespaces, QueryFailure, Scratch, MAX_COMPOUNDS};
use crate::init::EXC_CSS_SYNTAX_ERROR;
use crate::limits::NODE_SET_MAX;
use crate::selectors::ast::Lists;

/// The longest prefix or URI a CSS namespace binding takes: an XPath
/// binding's, so one Hash binds alike for `#css` and `#xpath`.
const BINDING_MAX_BYTES: usize = crate::xpath::limits::Limits::DEFAULT.max_string_bytes;

/// One call's `(selector, namespaces = nil)`, the argument list Nokogiri's
/// CSS methods take too - so one call works on either representation - read
/// and checked, ready to run over a representation's node.
///
/// The selector is a String or a Symbol, never converted (a `to_s` is the
/// caller's Ruby), and checked as text. The Hash declares the prefixes the
/// selector may use (`svg|path`, `[xlink|href]`), read and checked as
/// `#xpath`'s bindings are; a prefix it does not bind makes the selector
/// invalid. Its `"xmlns"` entry, if any, is the default namespace, as CSS
/// `@namespace` without a prefix sets one: a type or universal selector
/// without a prefix is then in it. Without one, a bare type selector matches
/// its name in any namespace, as in a browser.
///
/// Everything is read here, before the caller reads its document: the
/// queries below run no Ruby.
pub struct CssQuery {
    selector: Value,
    text: RubyText,
    bindings: crate::glue::namespaces::Bindings,
}

impl CssQuery {
    /// Read `(selector, namespaces = nil)` from `args`.
    pub fn scan(ruby: &Ruby, args: &[Value]) -> Result<CssQuery, Error> {
        let a =
            crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::positional(1, 1))?;
        let selector = a.required(0);
        let bindings = match a.optional(0) {
            None => Vec::new(),
            Some(ns) => {
                let Some(h) = RHash::from_value(ns) else {
                    return Err(Error::new(
                        crate::bridge::rb::Builtin::TypeError,
                        "namespaces must be a Hash of prefix => uri",
                    ));
                };
                crate::glue::namespaces::owned(ruby, h, BINDING_MAX_BYTES)?
            }
        };
        let text = ruby_verified_text(ruby, selector, "CSS selector")?;
        Ok(CssQuery {
            selector,
            text,
            bindings,
        })
    }

    /// Every element under `root` that matches, in document order (`#css`).
    pub fn select_all<'d, N: CssNode<'d>>(&self, root: N) -> Result<Vec<N>, Error>
    where
        N::Host: Kept,
    {
        self.compiled(|groups, scratch, ns| {
            crate::css_match::select_all(scratch, root, groups, ns)
        })?
        .map_err(|e| query_error(e, self.selector))
    }

    /// The first element under `root` that matches (`#at_css`).
    pub fn select_first<'d, N: CssNode<'d>>(&self, root: N) -> Result<Option<N>, Error>
    where
        N::Host: Kept,
    {
        self.compiled(|groups, scratch, ns| {
            crate::css_match::select_first(scratch, root, groups, ns)
        })?
        .map_err(|e| match_error(e, self.selector))
    }

    /// Whether `node` itself matches (`#matches?`).
    pub fn matches<'d, N: CssNode<'d>>(&self, node: N) -> Result<bool, Error>
    where
        N::Host: Kept,
    {
        self.compiled(|groups, scratch, ns| {
            crate::css_match::matches_any(scratch, groups, ns, node)
        })?
        .map_err(|e| match_error(e, self.selector))
    }

    /// `f` over the selector, compiled (and cached, `css_cache`), the host's
    /// kept scratch and the namespaces; a selector that does not parse is its
    /// exception.
    fn compiled<H: Kept, R>(
        &self,
        f: impl FnOnce(Lists<'_>, &mut Scratch<H>, &Namespaces<'_>) -> R,
    ) -> Result<R, Error> {
        self.with_namespaces(|ns| {
            crate::css_cache::with_compiled(self.text.as_bytes(), |groups, scratch| {
                f(groups, scratch, ns)
            })
            .map_err(|e| parse_error(e, self.selector))
        })?
    }

    /// The bindings as the matcher reads them, for `f`.
    fn with_namespaces<R>(&self, f: impl FnOnce(&Namespaces<'_>) -> R) -> Result<R, Error> {
        use falloc::{try_vec_with_capacity, VecGrow};
        if self.bindings.is_empty() {
            return Ok(f(&Namespaces::NONE));
        }
        let oom = || makiri_error("out of memory reading namespace bindings");
        let mut refs = try_vec_with_capacity(self.bindings.len()).ok_or_else(oom)?;
        let mut default = None;
        for (p, u) in &self.bindings {
            if p.as_slice() == Namespaces::DEFAULT_KEY {
                default = Some(u.as_slice());
            } else {
                refs.falloc_push((p.as_slice(), u.as_slice()))
                    .map_err(|()| oom())?;
            }
        }
        Ok(f(&Namespaces::new(&refs, default)))
    }
}

/// `Makiri::CSS::SyntaxError` naming `selector`.
fn syntax_error(selector: Value, reason: Option<&str>) -> Error {
    let reason = reason.unwrap_or("invalid CSS selector");
    Error::new(
        &EXC_CSS_SYNTAX_ERROR,
        format_args!("{reason}: {}", crate::bridge::ruby::AsText(selector)),
    )
}

/// A parse failure as the Ruby exception it maps to.
fn parse_error(err: ParseError, selector: Value) -> Error {
    match err {
        /* The parser, as Lexbor's which it ports, reports no reason. */
        ParseError::Syntax => syntax_error(selector, None),
        ParseError::Oom => makiri_error("out of memory parsing CSS selector"),
        ParseError::Busy => makiri_error("CSS selector engine is already in use"),
        ParseError::NotReady => makiri_error("failed to initialise CSS selector engine"),
    }
}

/// A whole-query failure ([`crate::css_match::select_all`]'s
/// error) as the Ruby exception it maps to.
fn query_error(err: QueryFailure, selector: Value) -> Error {
    match err {
        QueryFailure::Overflow => makiri_error(format_args!(
            "CSS result set exceeded the node limit ({NODE_SET_MAX})"
        )),
        QueryFailure::Match(e) => match_error(e, selector),
    }
}

/// As [`query_error`], for the entry points that cannot overflow the result
/// set ([`crate::css_match::select_first`], [`crate::css_match::matches_any`] -
/// one node each, never a `Vec`) and so only ever fail the other way.
///
/// `Unsupported` (the column combinator `||`, or `:current()`) is a
/// `Makiri::CSS::SyntaxError`, as a browser's `querySelectorAll` throws a
/// SyntaxError for both; its wording is the OLD engine's for the same case
/// ("CSS selector could not be run").
fn match_error(err: MatchFailure, selector: Value) -> Error {
    match err {
        MatchFailure::UndeclaredPrefix => {
            syntax_error(selector, Some("undeclared namespace prefix"))
        }
        MatchFailure::WorkExceeded => makiri_error("CSS query exceeded its work budget"),
        /* A construct the matcher does not run (`||`, `:current()`) is a
         * selector error, as a browser's SyntaxError is. */
        MatchFailure::Unsupported => syntax_error(selector, Some("CSS selector could not be run")),
        MatchFailure::TooComplex => makiri_error(format_args!(
            "CSS selector chain too complex (more than {MAX_COMPOUNDS} compounds)"
        )),
        MatchFailure::Oom => makiri_error("out of memory matching CSS selector"),
        MatchFailure::Internal => {
            crate::bridge::ruby::internal_error("CSS matcher reached a broken invariant")
        }
    }
}
