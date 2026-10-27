//! What both representations' `#css` / `#at_css` / `#matches?` share: the
//! argument list and its namespace bindings ([`CssArgs`]), and what a
//! rejected selector raises.
//!
//! One wording, so `Makiri::HTML` and `Makiri::XML` answer a bad selector the
//! same way: `"<reason>: <selector>"`, where the reason is the engine's when it
//! has one ("undeclared namespace prefix") and a plain "invalid CSS selector"
//! otherwise.

#![forbid(unsafe_code)]

use crate::bridge::interrupt::Pass;
use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::css_cache::ParseError;
use crate::css_match::{MatchFailure, Namespaces, QueryFailure, MAX_COMPOUNDS};
use crate::init::EXC_CSS_SYNTAX_ERROR;
use crate::limits::NODE_SET_MAX;

/// `(selector, namespaces = nil)`, the argument list Nokogiri's CSS methods
/// take too - so one call works on either representation.
///
/// The Hash declares the prefixes the selector may use (`svg|path`,
/// `[xlink|href]`), read and checked as `#xpath`'s bindings are; a prefix it
/// does not bind makes the selector invalid. Its `"xmlns"` entry, if any, is
/// the default namespace, as CSS `@namespace` without a prefix sets one: a
/// type or universal selector without a prefix is then in it. Without one, a
/// bare type selector matches its name in any namespace, as in a browser.
pub struct CssArgs {
    pub selector: Value,
    bindings: crate::glue::query::NamespaceBindings,
}

impl CssArgs {
    /// Read `(selector, namespaces = nil)` from `args`.
    pub fn scan(ruby: &Ruby, args: &[Value]) -> Result<CssArgs, Error> {
        let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::positional(1, 1))?;
        CssArgs::new(
            ruby,
            a.required(0),
            a.optional(0).unwrap_or_else(|| ruby.qnil().as_value()),
        )
    }

    /// The selector and its namespaces (`nil` or a Hash).
    pub fn new(ruby: &Ruby, selector: Value, ns: Value) -> Result<CssArgs, Error> {
        let mut bindings = Vec::new();
        if !ns.is_nil() {
            let Some(h) = RHash::from_value(ns) else {
                return Err(Error::new(
                    crate::bridge::rb::Builtin::TypeError,
                    "namespaces must be a Hash of prefix => uri",
                ));
            };
            bindings = crate::glue::query::namespace_bindings(ruby, h)?;
        }
        Ok(CssArgs { selector, bindings })
    }

    /// The bindings as the matcher reads them, for `f`.
    pub fn with_namespaces<R>(&self, f: impl FnOnce(&Namespaces<'_>) -> R) -> Result<R, Error> {
        use crate::falloc::{try_vec_with_capacity, VecGrow};
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
pub fn syntax_error(selector: Value, reason: Option<&str>) -> Error {
    let reason = reason.unwrap_or("invalid CSS selector");
    Error::new(
        &EXC_CSS_SYNTAX_ERROR,
        format_args!("{reason}: {}", crate::bridge::ruby::AsText(selector)),
    )
}

/// A parse failure as the Ruby exception it maps to.
pub fn parse_error(err: ParseError, selector: Value) -> Error {
    match err {
        /* Lexbor's parser reports no reason of its own. */
        ParseError::Syntax => syntax_error(selector, None),
        ParseError::Oom => makiri_error("out of memory parsing CSS selector"),
        ParseError::Busy => makiri_error("CSS selector engine is already in use"),
        ParseError::NotReady => makiri_error("failed to initialise CSS selector engine"),
    }
}

/// A whole-query failure ([`crate::css_match::select_all`]'s
/// error) as the Ruby exception it maps to.
pub fn query_error(err: QueryFailure, selector: Value) -> Error {
    match err {
        QueryFailure::Overflow => makiri_error(format_args!(
            "CSS result set exceeded the node limit ({NODE_SET_MAX})"
        )),
        QueryFailure::Match(e) => match_error(e, selector),
    }
}

/// What a matcher entry point came to, as a [`Pass`] of `interruptible`: its
/// answer, "stopped for an interrupt" (which is no failure, and is what every
/// CSS entry point must say for it - one that forgot would raise it at the
/// caller), or the exception its failure maps to.
pub fn pass_match<T>(result: Result<T, MatchFailure>, selector: Value) -> Result<Pass<T>, Error> {
    match result {
        Ok(answer) => Ok(Pass::Done(answer)),
        Err(MatchFailure::Interrupted) => Ok(Pass::Interrupted),
        Err(e) => Err(match_error(e, selector)),
    }
}

/// [`pass_match`] for the whole-query entry point ([`css_match::select_all`]).
pub fn pass_query<T>(result: Result<T, QueryFailure>, selector: Value) -> Result<Pass<T>, Error> {
    match result {
        Ok(answer) => Ok(Pass::Done(answer)),
        Err(QueryFailure::Match(MatchFailure::Interrupted)) => Ok(Pass::Interrupted),
        Err(e) => Err(query_error(e, selector)),
    }
}

/// As [`query_error`], for the entry points that cannot overflow the result
/// set ([`css_match::select_first`], [`css_match::matches_any`] -
/// one node each, never a `Vec`) and so only ever fail the other way.
///
/// `Unsupported` (the column combinator `||`, or `:current()`) is a
/// `Makiri::CSS::SyntaxError`, as a browser's `querySelectorAll` throws a
/// SyntaxError for both; its wording is the OLD engine's for the same case
/// ("CSS selector could not be run").
pub fn match_error(err: MatchFailure, selector: Value) -> Error {
    match err {
        MatchFailure::UndeclaredPrefix => {
            syntax_error(selector, Some("undeclared namespace prefix"))
        }
        MatchFailure::WorkExceeded => makiri_error("CSS query exceeded its work budget"),
        /* `bridge::interrupt::interruptible` turns this into another pass; one
         * that reaches here was not asked to stop. */
        MatchFailure::Interrupted => makiri_error("CSS query was interrupted"),
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
