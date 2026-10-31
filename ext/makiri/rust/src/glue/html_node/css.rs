//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine lives in [`crate::css_match`] (matching,
//! over the typed adapter, non-recursive) plus [`crate::css_cache`]
//! (parsing, with the parser port [`crate::selectors`], AND caching the
//! parse across repeat calls with the same selector string), neither of
//! which knows about Ruby. The arguments, the selector's check and every
//! failure's exception are [`CssQuery`]'s, shared with XML; this module hands
//! it the receiver's node and fills the NodeSet (`css`) or wraps the one node
//! `at_css` found.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::method;

use super::HtmlSelf;
use crate::bridge::html::wrap_html_node;
use crate::bridge::node_set::node_set_from;
use crate::glue::css::CssQuery;
use crate::html_dom::html::RawNode;
use crate::init::MOD_HTML_NODE_METHODS;

/// `Node#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Value, Error> {
    let query = CssQuery::scan(ruby, args)?;
    /* Held while the set is built (`node_set_from`): the matched nodes
     * borrow it. */
    let view = this.read()?;
    let nodes = query.select_all(view.node())?;
    node_set_from(
        ruby,
        this.document(),
        nodes.into_iter().map(|n| RawNode::from(n).into()),
    )
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<Option<Value>, Error> {
    let query = CssQuery::scan(ruby, args)?;
    let found = query.select_first(this.read()?.node())?.map(RawNode::from);
    found
        .map(|n| wrap_html_node(ruby, n, this.document()))
        .transpose()
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri. A non-element node (there is no CSS
/// selector, not even `*`, that an element-only engine can match it with -
/// see `css_match`'s `Simple::Universal` fix) never matches; the selector is
/// still compiled, so one the matcher refuses is refused whatever node it is
/// asked about (`css_match::matches_any`).
fn matches(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<bool, Error> {
    let query = CssQuery::scan(ruby, args)?;
    query.matches(this.read()?.node())
}

/// From `Init_makiri`.
pub fn init_css(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.defined(ruby)?;
    m.define_method("css", method!(css, -1))?;
    m.define_method("at_css", method!(at_css, -1))?;
    m.define_method("matches?", method!(matches, -1))?;
    Ok(())
}
