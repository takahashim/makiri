//! `Node#to_html` / `#to_s` / `#outer_html` and `#inner_html`.
//!
//! Reads the receiver and the `pretty:` option, picks the tree or deep
//! serializer, and copies the owned bytes into a String. The serializer
//! and its buffer live in [`crate::html_dom::serialize`], which does not know
//! about Ruby.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, RString, Ruby, Value};
use crate::method;

use super::HtmlSelf;
use crate::bridge::args::Kwargs;
use crate::bridge::ruby::makiri_error;
use crate::html_dom::html::{HtmlNode, NodeType};
use crate::html_dom::serialize::{serialize, Layout, Scope};
use crate::init::MOD_HTML_NODE_METHODS;

/// The optional `pretty:` and `encoding:` keywords; any other is
/// `ArgumentError`, so a misspelt one is not quietly the default.
///
/// The serializer writes UTF-8, so `encoding:` - which code written for
/// Nokogiri passes, most often as `"UTF-8"` - is taken when it names UTF-8
/// and refused otherwise. Ignoring it answered a UTF-8 String to a caller
/// that asked for another encoding.
fn layout_opt(ruby: &Ruby, args: &[Value]) -> Result<Layout, Error> {
    let kw = Kwargs::scan(ruby, args)?;
    let kw = kw.only(ruby, &["pretty", "encoding"])?;
    if let Some(v) = kw.value(ruby, "encoding")? {
        /* Read as Ruby reads an encoding argument: an Encoding, or a String
         * or Symbol naming one; an unknown name is Ruby's ArgumentError. */
        let e = crate::bridge::string::to_encoding(ruby, v)?;
        if e != crate::bridge::string::Encoding::utf8(ruby) {
            let name = core::str::from_utf8(e.name()).unwrap_or("another encoding");
            return Err(crate::bridge::ruby::arg_error(format_args!(
                "HTML is serialized as UTF-8 alone, not {name}; encode the result with \
                 String#encode"
            )));
        }
    }
    Ok(if kw.flag(ruby, "pretty")? {
        Layout::Pretty
    } else {
        Layout::Plain
    })
}

/// The receiver's serialization as a UTF-8 String, or `Makiri::Error` on a
/// serializer failure.
///
/// An Attr serializes to the empty string, as DOM Parsing's algorithm and
/// Firefox have it (Chromium and WebKit write the escaped value) and as an
/// XML Attr does; the serializer itself has no markup for one, and refused.
fn render(ruby: &Ruby, node: HtmlNode<'_>, scope: Scope, layout: Layout) -> Result<RString, Error> {
    if node.node_type() == NodeType::Attribute {
        return crate::bridge::string::utf8_str(ruby, "");
    }
    let buf =
        serialize(node, scope, layout).ok_or_else(|| makiri_error("HTML serialization failed"))?;
    /* The serializer emits UTF-8, so the String is tagged UTF-8 rather than
     * built as binary and re-tagged; protected, so a failed allocation drops
     * `buf` rather than leaking it. */
    crate::bridge::string::utf8_str_trusting(ruby, buf.as_slice())
}

/// Outer HTML: the node itself plus its descendants. `pretty: true` indents.
fn to_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    let layout = layout_opt(ruby, args)?;
    /* A document fragment has no tag of its own, so its "outer" is its
     * children: the inner scope is the right one (the outer one rejects
     * a fragment node). */
    let view = this.read()?;
    let node = view.node();
    let scope = if node.node_type() == NodeType::DocumentFragment {
        Scope::Inner
    } else {
        Scope::Outer
    };
    /* A failed allocation of the String is `Err`, so the read may stay. */
    render(ruby, node, scope, layout)
}

/// Inner HTML: the node's children, without the node's own tag.
///
/// WHATWG special-cases a `<template>`: its inner HTML is its template contents,
/// a separate fragment rather than the element's (empty) children - the same
/// node `Element#content_fragment` exposes and `#to_html` serializes, so the
/// three agree.
fn inner_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    let layout = layout_opt(ruby, args)?;
    let view = this.read()?;
    let node = view.node();
    let target = node.template_content().unwrap_or(node);
    render(ruby, target, Scope::Inner, layout)
}

/// From `Init_makiri`.
pub fn init_serialize(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.defined(ruby)?;
    for name in ["to_html", "to_s", "outer_html"] {
        m.define_method(name, method!(to_html, -1))?;
    }
    m.define_method("inner_html", method!(inner_html, -1))?;
    Ok(())
}
