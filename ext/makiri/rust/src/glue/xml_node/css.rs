//! CSS over XML: the private `_css` / `_at_css` / `_css_matches` primitives the
//! Ruby `#css` / `#at_css` / `#matches?` call (`lib/makiri/xml/node_methods.rb`).
//!
//! The selector runs through the same matcher as HTML's (`css_match`), over
//! the XML arena (`xml::css`), with the same compiled-selector cache, the
//! same namespace bindings ([`CssArgs`]) and the same exceptions. What
//! differs is the document's: no XML document is an HTML document, so names
//! and values compare as written, an XHTML element included.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};

use super::{wrap, XmlSelf};
use crate::bridge::gvl::held;
use crate::bridge::node_set::node_set_from;
use crate::bridge::string::ruby_verified_text;
use crate::css_match;
use crate::glue::css::{match_error, parse_error, query_error, CssArgs};
use crate::init::MOD_XML_NODE_METHODS;
use crate::lexbor::selector_cache;
use crate::xml::css::XmlNode;

/// `#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let args = CssArgs::new(ruby, selector, ns)?;
        let selector = args.selector;
        let sv = ruby_verified_text(selector, "CSS selector")?;
        let gvl = held(ruby);
        let root = XmlNode::new(this.doc_ref(), this.id);
        let matched = args
            .with_namespaces(|ns| {
                selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
                    css_match::select_all(scratch, root, groups, ns)
                })
            })?
            .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let nodes = matched.map_err(|e| query_error(e, selector))?;
        node_set_from(this.document, nodes.into_iter().map(|n| n.id().into()))
    })
}

/// `#at_css`: the first matching descendant, or nil.
fn at_css(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let args = CssArgs::new(ruby, selector, ns)?;
        let selector = args.selector;
        let sv = ruby_verified_text(selector, "CSS selector")?;
        let gvl = held(ruby);
        let root = XmlNode::new(this.doc_ref(), this.id);
        let matched = args
            .with_namespaces(|ns| {
                selector_cache::with_compiled(&gvl, sv.as_bytes(), |groups, scratch| {
                    css_match::select_first(scratch, root, groups, ns)
                })
            })?
            .map_err(|e| parse_error(e, selector))?;
        drop(sv);
        let found = matched.map_err(|e| match_error(e, selector))?;
        found.map(|n| wrap(n.id(), this.document)).transpose()
    })
}

/// `#matches?`: does THIS node match? Its ancestors and siblings take part,
/// wherever the node is.
fn css_matches(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let args = CssArgs::new(ruby, selector, ns)?;
        let selector = args.selector;
        let sv = ruby_verified_text(selector, "CSS selector")?;
        let gvl = held(ruby);
        let node = XmlNode::new(this.doc_ref(), this.id);
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

/// The private primitives, on the XML node-method module. From `Init_makiri`.
pub fn init_xml_css() -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.defined()?;
    m.define_private_method("_css", method!(css, 2))?;
    m.define_private_method("_at_css", method!(at_css, 2))?;
    m.define_private_method("_css_matches", method!(css_matches, 2))?;
    Ok(())
}
