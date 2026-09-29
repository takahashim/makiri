//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine lives in [`crate::lexbor::css_match`] (matching,
//! over the typed adapter, non-recursive) plus [`crate::lexbor::selector_cache`]
//! (parsing AND caching - Lexbor's own C parser, kept warm across repeat
//! calls with the same selector string, its own process-global engine
//! separate from [`crate::lexbor::css_parser`]'s), neither of which knows
//! about Ruby. This module verifies the selector, maps an engine failure to
//! its exception, and fills the NodeSet (`css`) or wraps the one node
//! `at_css` found.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RHash, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::gvl::held;
use crate::bridge::html::wrap_html_node;
use crate::bridge::node_set::node_set_from;
use crate::bridge::ruby::makiri_error;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::init::MOD_HTML_NODE_METHODS;
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::css_match::{self, MatchFailure, QueryFailure, MAX_COMPOUNDS};
use crate::lexbor::css_parser::ParseError;
use crate::lexbor::selector_cache;
use crate::limits::NODE_SET_MAX;

/// A parse failure as the Ruby exception it maps to.
fn parse_error(err: ParseError, selector: Value) -> Error {
    match err {
        /* Lexbor's parser reports no reason of its own. */
        ParseError::Syntax => crate::glue::css::syntax_error(selector, None),
        ParseError::Oom => makiri_error("out of memory parsing CSS selector"),
        ParseError::Busy => makiri_error("CSS selector engine is already in use"),
        ParseError::NotReady => makiri_error("failed to initialise CSS selector engine"),
    }
}

/// A whole-query failure ([`crate::lexbor::css_match::select_all`]'s
/// error) as the Ruby exception it maps to.
fn query_error(err: QueryFailure) -> Error {
    match err {
        QueryFailure::Overflow => makiri_error(format!(
            "CSS result set exceeded the node limit ({NODE_SET_MAX})"
        )),
        QueryFailure::WorkExceeded => match_error(MatchFailure::WorkExceeded),
        QueryFailure::Unsupported => match_error(MatchFailure::Unsupported),
        QueryFailure::TooComplex => match_error(MatchFailure::TooComplex),
        QueryFailure::Oom => match_error(MatchFailure::Oom),
    }
}

/// As [`query_error`], for the entry points that cannot overflow the result
/// set ([`css_match::select_first`], [`css_match::matches_any`] -
/// one node each, never a `Vec`) and so only ever fail the other way.
///
/// The message for `Unsupported` (the column combinator `||`, or
/// `:lexbor-contains()`) matches the OLD engine's own wording for the same
/// case (`SelectError::Traversal`'s "CSS selector could not be run") on
/// purpose: it is the same fact - this engine could not run the selector
/// either - not a new one.
fn match_error(err: MatchFailure) -> Error {
    match err {
        MatchFailure::WorkExceeded => makiri_error("CSS query exceeded its work budget"),
        MatchFailure::Unsupported => makiri_error("CSS selector could not be run"),
        MatchFailure::TooComplex => makiri_error(format!(
            "CSS selector chain too complex (more than {MAX_COMPOUNDS} compounds)"
        )),
        MatchFailure::Oom => makiri_error("out of memory matching CSS selector"),
    }
}

/// The selector after the strict text check. The engine reads its bytes and
/// runs no Ruby, so the view stays valid for the query.
fn selector_text(selector: Value) -> Result<RubyText, Error> {
    ruby_verified_text(selector, "CSS selector")
}

/// `(selector, namespaces = nil)`, the argument list `Makiri::XML`'s CSS
/// methods and Nokogiri's take - so one call works on either representation.
///
/// The bindings are ACCEPTED AND UNUSED here: the matcher resolves a
/// selector's names itself, and its prefix handling is loose (`svg|path` and
/// `path` match the same elements whatever a caller binds). Refusing them
/// instead would break the common `node.css(selector, ns)` written for both
/// representations, and would protect nothing - the answer is the same with or
/// without them. See NOKOGIRI_DIFFERENCES.md; `#xpath` is where a prefix is
/// resolved against real bindings.
fn css_args(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    if let Some(ns) = a.optional.0.filter(|v| !v.is_nil()) {
        if RHash::from_value(ns).is_none() {
            return Err(Error::new(
                ruby.exception_type_error(),
                "namespaces must be a Hash of prefix => uri",
            ));
        }
    }
    Ok(a.required.0)
}

/// `Node#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let selector = css_args(ruby, args)?;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let matched = selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
            css_match::select_all_in(scratch, this.node(), groups)
        })
        .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let nodes = matched.map_err(query_error)?;
        node_set_from(
            this.document,
            nodes.into_iter().map(|n| RawNode::from(n).into()),
        )
    })
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let selector = css_args(ruby, args)?;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let matched = selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
            css_match::select_first_in(scratch, this.node(), groups)
        })
        .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let found = matched.map_err(match_error)?;
        Ok(found.map(|n| wrap_html_node(RawNode::from(n), this.document)))
    })
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri. A non-element node (there is no CSS
/// selector, not even `*`, that an element-only engine can match it with -
/// see `css_match`'s `Simple::Universal` fix) never matches, without
/// asking the engine.
fn matches(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let selector = css_args(ruby, args)?;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let element = this.node().element();
        let matched = selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
            match element {
                Some(el) => css_match::matches_any_in(scratch, groups, el),
                // Still compiled: a selector the matcher refuses is refused
                // whatever node it is asked about.
                None => css_match::check_compiles(scratch, groups).map(|()| false),
            }
        })
        .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        matched.map_err(match_error)
    })
}

/// From `Init_makiri`.
pub fn init_css() -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.defined()?;
    m.define_method("css", method!(css, -1))?;
    m.define_method("at_css", method!(at_css, -1))?;
    m.define_method("matches?", method!(matches, -1))?;
    Ok(())
}
