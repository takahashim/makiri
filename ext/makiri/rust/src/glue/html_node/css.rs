//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine lives in [`crate::css_match`] (matching,
//! over the typed adapter, non-recursive) plus [`crate::lexbor::selector_cache`]
//! (parsing, with the parser port [`crate::selectors`], AND caching the
//! parse across repeat calls with the same selector string), neither of
//! which knows about Ruby. This module verifies the selector, maps an engine failure to
//! its exception, and fills the NodeSet (`css`) or wraps the one node
//! `at_css` found.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::gvl::held;
use crate::bridge::html::wrap_html_node;
use crate::bridge::node_set::node_set_from;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::css_match;
use crate::glue::css::{match_error, parse_error, query_error, CssArgs};
use crate::html_dom::html::RawNode;
use crate::init::MOD_HTML_NODE_METHODS;
use crate::lexbor::selector_cache;

/// The selector after the strict text check. The engine reads its bytes and
/// runs no Ruby, so the view stays valid for the query.
fn selector_text(selector: Value) -> Result<RubyText, Error> {
    ruby_verified_text(selector, "CSS selector")
}

/// `Node#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let args = CssArgs::scan(ruby, args)?;
        let selector = args.selector;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let matched = args
            .with_namespaces(|ns| {
                selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
                    css_match::select_all(scratch, this.node(), groups, ns)
                })
            })?
            .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let nodes = matched.map_err(|e| query_error(e, selector))?;
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
        let args = CssArgs::scan(ruby, args)?;
        let selector = args.selector;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let matched = args
            .with_namespaces(|ns| {
                selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
                    css_match::select_first(scratch, this.node(), groups, ns)
                })
            })?
            .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let found = matched.map_err(|e| match_error(e, selector))?;
        found
            .map(|n| wrap_html_node(RawNode::from(n), this.document))
            .transpose()
    })
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri. A non-element node (there is no CSS
/// selector, not even `*`, that an element-only engine can match it with -
/// see `css_match`'s `Simple::Universal` fix) never matches; the selector is
/// still compiled, so one the matcher refuses is refused whatever node it is
/// asked about (`css_match::matches_any`).
fn matches(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let args = CssArgs::scan(ruby, args)?;
        let selector = args.selector;
        let sv = selector_text(selector)?;
        let gvl = held(ruby);
        let node = this.node();
        let matched = args
            .with_namespaces(|ns| {
                selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
                    css_match::matches_any(scratch, groups, ns, node)
                })
            })?
            .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        matched.map_err(|e| match_error(e, selector))
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
