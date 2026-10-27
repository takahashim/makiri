//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine lives in [`crate::css_match`] (matching,
//! over the typed adapter, non-recursive) plus [`crate::css_cache`]
//! (parsing, with the parser port [`crate::selectors`], AND caching the
//! parse across repeat calls with the same selector string), neither of
//! which knows about Ruby. This module verifies the selector, maps an engine failure to
//! its exception, and fills the NodeSet (`css`) or wraps the one node
//! `at_css` found.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::method;

use super::HtmlSelf;
use crate::bridge::html::wrap_html_node;
use crate::bridge::interrupt::interruptible;
use crate::bridge::node_set::node_set_from;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::css_cache;
use crate::css_match;
use crate::glue::css::{parse_error, pass_match, pass_query, CssArgs};
use crate::html_dom::html::RawNode;
use crate::init::MOD_HTML_NODE_METHODS;

/// The selector after the strict text check. The engine reads its bytes and
/// runs no Ruby, so the view stays valid for the query.
fn selector_text(ruby: &Ruby, selector: Value) -> Result<RubyText, Error> {
    ruby_verified_text(ruby, selector, "CSS selector")
}

/// `Node#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Value, Error> {
    let args = CssArgs::scan(ruby, args)?;
    let selector = args.selector;
    let sv = selector_text(ruby, selector)?;
    /* Read once the namespaces are converted (their Hash is Ruby), and
     * held while the set is built (`node_set_from`): the matched nodes
     * borrow it. */
    let found = args.with_namespaces(|ns| {
        /* One pass per attempt: the read is taken and dropped inside it, so an
         * interrupt is delivered with none held. */
        interruptible(ruby, || {
            let view = this.read()?;
            let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
                css_match::select_all(scratch, view.node(), groups, ns)
            })
            .map_err(|e| parse_error(e, selector))?;
            pass_query(matched, selector)?.then(|nodes| {
                node_set_from(
                    ruby,
                    this.document,
                    nodes.into_iter().map(|n| RawNode::from(n).into()),
                )
            })
        })
    })?;
    drop(sv);
    found
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Option<Value>, Error> {
    let args = CssArgs::scan(ruby, args)?;
    let selector = args.selector;
    let sv = selector_text(ruby, selector)?;
    let found = args.with_namespaces(|ns| {
        interruptible(ruby, || {
            let view = this.read()?;
            let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
                css_match::select_first(scratch, view.node(), groups, ns)
            })
            .map_err(|e| parse_error(e, selector))?;
            pass_match(matched, selector).map(|pass| pass.map(|n| n.map(RawNode::from)))
        })
    })??;
    drop(sv);
    found
        .map(|n| wrap_html_node(ruby, n, this.document))
        .transpose()
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri. A non-element node (there is no CSS
/// selector, not even `*`, that an element-only engine can match it with -
/// see `css_match`'s `Simple::Universal` fix) never matches; the selector is
/// still compiled, so one the matcher refuses is refused whatever node it is
/// asked about (`css_match::matches_any`).
fn matches(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<bool, Error> {
    let args = CssArgs::scan(ruby, args)?;
    let selector = args.selector;
    let sv = selector_text(ruby, selector)?;
    let matched = args.with_namespaces(|ns| {
        interruptible(ruby, || {
            let view = this.read()?;
            let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
                css_match::matches_any(scratch, groups, ns, view.node())
            })
            .map_err(|e| parse_error(e, selector))?;
            pass_match(matched, selector)
        })
    })?;
    drop(sv);
    matched
}

/// From `Init_makiri`.
pub fn init_css(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.defined(ruby)?;
    m.define_method("css", method!(css, -1))?;
    m.define_method("at_css", method!(at_css, -1))?;
    m.define_method("matches?", method!(matches, -1))?;
    Ok(())
}
