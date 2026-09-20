//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine, its process-global cache and its Lexbor callbacks live
//! in [`crate::lexbor::selectors`], which does not know about Ruby. This module
//! verifies the selector, maps an engine failure to its exception, and fills the
//! NodeSet (`css`) or wraps the one node `at_css` found.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::gvl::held;
use crate::bridge::html::wrap_html_node;
use crate::bridge::node_set::node_set_from;
use crate::bridge::ruby::makiri_error;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::init::{EXC_CSS_SYNTAX_ERROR, MOD_HTML_NODE_METHODS};
use crate::lexbor::selectors::{matches_node, select_all, select_first, SelectError};
use crate::limits::NODE_SET_MAX;

/// An engine failure as the Ruby exception it maps to.
fn select_error(err: SelectError, selector: Value) -> Error {
    match err {
        SelectError::Syntax => Error::new(
            EXC_CSS_SYNTAX_ERROR.exception(),
            format!("invalid CSS selector: {selector}"),
        ),
        SelectError::Overflow => makiri_error(format!(
            "CSS result set exceeded the node limit ({NODE_SET_MAX})"
        )),
        SelectError::CollectOom => makiri_error("out of memory collecting CSS results"),
        SelectError::CacheOom => makiri_error("out of memory caching CSS selector"),
        SelectError::Unavailable => makiri_error("failed to initialise CSS selector engine"),
        SelectError::Busy => makiri_error("CSS selector engine is already in use"),
    }
}

/// The selector after the strict text check. The engine reads its bytes and
/// runs no Ruby, so the view stays valid for the query.
fn selector_text(selector: Value) -> Result<RubyText, Error> {
    ruby_verified_text(selector, c"CSS selector")
}

/// `Node#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: HtmlSelf, selector: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let sv = selector_text(selector)?;
        let nodes = select_all(&held(ruby), this.raw(), sv.as_verified().as_bytes())
            .map_err(|e| select_error(e, selector))?;
        drop(sv);
        node_set_from(this.document, nodes.iter().map(|n| n.as_ptr()))
    })
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(ruby: &Ruby, this: HtmlSelf, selector: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let sv = selector_text(selector)?;
        let found = select_first(&held(ruby), this.raw(), sv.as_verified().as_bytes())
            .map_err(|e| select_error(e, selector))?;
        drop(sv);
        Ok(found.map_or_else(
            || ruby.qnil().as_value(),
            |n| wrap_html_node(n, this.document),
        ))
    })
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri.
fn matches(ruby: &Ruby, this: HtmlSelf, selector: Value) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let sv = selector_text(selector)?;
        matches_node(&held(ruby), this.raw(), sv.as_verified().as_bytes())
            .map_err(|e| select_error(e, selector))
    })
}

/// From `Init_makiri`.
pub fn init_css() {
    let m = MOD_HTML_NODE_METHODS.module();
    m.define_method("css", method!(css, 1)).expect("Node#css");
    m.define_method("at_css", method!(at_css, 1))
        .expect("Node#at_css");
    m.define_method("matches?", method!(matches, 1))
        .expect("Node#matches?");
}
