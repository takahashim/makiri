//! CSS over XML: `Node#css` / `#at_css` / `#matches?` on
//! `Makiri::XML::NodeMethods`.
//!
//! The selector runs through the same matcher as HTML's (`css_match`), over
//! the XML arena (`xml::css`), with the same compiled-selector cache, the
//! same argument list and namespace bindings and the same exceptions
//! ([`CssQuery`]). What differs is the document's: no XML document is an HTML
//! document, so names and values compare as written, an XHTML element
//! included - as a browser's `querySelectorAll` matches them in an XML
//! document. A type selector without a prefix matches its name in any
//! namespace, and the context node's ancestors take part in the match while
//! only its descendants are returned. A `{prefix => uri}` Hash declares the
//! prefixes the selector may use; its `"xmlns"` entry, if any, is the default
//! namespace a bare type selector is then in:
//!
//! ```text
//! doc.css("entry")                    # <entry> in any namespace
//! doc.css("a|entry", "a" => uri)      # <entry> in uri
//! doc.css("entry", "xmlns" => uri)    # the same, as a default
//! ```

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::method;

use super::{wrap, XmlSelf};
use crate::bridge::node_set::node_set_from;
use crate::glue::css::CssQuery;
use crate::init::MOD_XML_NODE_METHODS;
use crate::xml::css::XmlNode;

/// `#css`: every matching descendant, in document order.
fn css(ruby: &Ruby, this: XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let query = CssQuery::scan(ruby, args)?;
    /* Held while the set is built (`node_set_from`): the matched nodes
     * borrow it. */
    let d = this.read()?;
    let nodes = query.select_all(XmlNode::new(&d, this.id))?;
    node_set_from(
        ruby,
        this.document(),
        nodes.into_iter().map(|n| n.id().into()),
    )
}

/// `#at_css`: the first matching descendant, or nil.
fn at_css(ruby: &Ruby, this: XmlSelf, args: &[Value]) -> Result<Option<Value>, Error> {
    let query = CssQuery::scan(ruby, args)?;
    let found = query
        .select_first(XmlNode::new(&*this.read()?, this.id))?
        .map(|n| n.id());
    found.map(|n| wrap(ruby, n, this.document())).transpose()
}

/// `#matches?`: does THIS node match? Its ancestors and siblings take part,
/// wherever the node is.
fn matches(ruby: &Ruby, this: XmlSelf, args: &[Value]) -> Result<bool, Error> {
    let query = CssQuery::scan(ruby, args)?;
    query.matches(XmlNode::new(&*this.read()?, this.id))
}

/// From `Init_makiri`.
pub fn init_xml_css(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.defined(ruby)?;
    m.define_method("css", method!(css, -1))?;
    m.define_method("at_css", method!(at_css, -1))?;
    m.define_method("matches?", method!(matches, -1))?;
    Ok(())
}
