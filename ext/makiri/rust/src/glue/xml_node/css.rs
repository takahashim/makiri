//! CSS over XML: the private `_css` / `_at_css` / `_css_matches` primitives the
//! Ruby `#css` / `#at_css` / `#matches?` call (`lib/makiri/xml/node_methods.rb`).
//!
//! The selector runs through the same matcher as HTML's (`css_match`), over
//! the XML arena (`xml::css`), with the same compiled-selector cache, the
//! same namespace bindings ([`CssArgs`]) and the same exceptions. What
//! differs is the document's: no XML document is an HTML document, so names
//! and values compare as written, an XHTML element included.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::method;

use super::{wrap, XmlSelf};
use crate::bridge::node_set::node_set_from;
use crate::bridge::string::ruby_verified_text;
use crate::css_cache;
use crate::css_match;
use crate::glue::css::{match_error, parse_error, query_error, CssArgs};
use crate::init::MOD_XML_NODE_METHODS;
use crate::xml::css::XmlNode;

/// `#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<Value, Error> {
    let args = CssArgs::new(ruby, selector, ns)?;
    let selector = args.selector;
    let sv = ruby_verified_text(ruby, selector, "CSS selector")?;
    /* Read once the namespaces are converted (their Hash is Ruby), and
     * held while the set is built (`node_set_from`): the matched nodes
     * borrow it. */
    let found = args.with_namespaces(|ns| {
        let d = this.read()?;
        let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
            css_match::select_all(scratch, XmlNode::new(&d, this.id), groups, ns)
        })
        .map_err(|e| parse_error(e, selector))?;
        let nodes = matched.map_err(|e| query_error(e, selector))?;
        node_set_from(
            ruby,
            this.document(),
            nodes.into_iter().map(|n| n.id().into()),
        )
    })?;
    drop(sv);
    found
}

/// `#at_css`: the first matching descendant, or nil.
fn at_css(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<Option<Value>, Error> {
    let args = CssArgs::new(ruby, selector, ns)?;
    let selector = args.selector;
    let sv = ruby_verified_text(ruby, selector, "CSS selector")?;
    let found = args.with_namespaces(|ns| {
        let d = this.read()?;
        let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
            css_match::select_first(scratch, XmlNode::new(&d, this.id), groups, ns)
        })
        .map_err(|e| parse_error(e, selector))?;
        Ok::<_, Error>(
            matched
                .map_err(|e| match_error(e, selector))?
                .map(|n| n.id()),
        )
    })??;
    drop(sv);
    found.map(|n| wrap(ruby, n, this.document())).transpose()
}

/// `#matches?`: does THIS node match? Its ancestors and siblings take part,
/// wherever the node is.
fn css_matches(ruby: &Ruby, this: XmlSelf, selector: Value, ns: Value) -> Result<bool, Error> {
    let args = CssArgs::new(ruby, selector, ns)?;
    let selector = args.selector;
    let sv = ruby_verified_text(ruby, selector, "CSS selector")?;
    let matched = args.with_namespaces(|ns| {
        let d = this.read()?;
        let matched = css_cache::with_compiled(sv.as_bytes(), |groups, scratch| {
            css_match::matches_any(scratch, groups, ns, XmlNode::new(&d, this.id))
        })
        .map_err(|e| parse_error(e, selector))?;
        matched.map_err(|e| match_error(e, selector))
    })?;
    drop(sv);
    matched
}

/// The private primitives, on the XML node-method module. From `Init_makiri`.
pub fn init_xml_css(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.defined(ruby)?;
    m.define_private_method("_css", method!(css, 2))?;
    m.define_private_method("_at_css", method!(at_css, 2))?;
    m.define_private_method("_css_matches", method!(css_matches, 2))?;
    Ok(())
}
