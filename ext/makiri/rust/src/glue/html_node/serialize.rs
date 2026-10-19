//! `Node#to_html` / `#to_s` / `#outer_html` and `#inner_html`.
//!
//! Reads the receiver and the `pretty:` option, picks the tree or deep
//! serializer, and copies the owned bytes into a String. The Lexbor callbacks
//! and the buffer live in [`crate::html_dom::serialize`], which does not know
//! about Ruby.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RString, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::ruby::makiri_error;
use crate::glue::kwargs::Kwargs;
use crate::html_dom::html::{HtmlNode, NodeType};
use crate::html_dom::serialize::{serialize, Layout, Scope};
use crate::init::MOD_HTML_NODE_METHODS;

/// The optional `pretty:` keyword.
fn layout_opt(ruby: &Ruby, args: &[Value]) -> Result<Layout, Error> {
    Ok(if Kwargs::scan(args)?.flag(ruby, "pretty") {
        Layout::Pretty
    } else {
        Layout::Plain
    })
}

/// The receiver's serialization as a UTF-8 String, or `Makiri::Error` on a
/// Lexbor status failure.
fn render(ruby: &Ruby, node: HtmlNode<'_>, scope: Scope, layout: Layout) -> Result<RString, Error> {
    let buf =
        serialize(node, scope, layout).ok_or_else(|| makiri_error("HTML serialization failed"))?;
    /* Lexbor emits UTF-8, so the String is tagged UTF-8 rather than built as
     * binary and re-tagged. */
    Ok(ruby.enc_str_new(buf.as_slice(), ruby.utf8_encoding()))
}

/// Outer HTML: the node itself plus its descendants. `pretty: true` indents.
fn to_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let layout = layout_opt(ruby, args)?;
        /* A document fragment has no tag of its own, so its "outer" is its
         * children: the inner scope is the right one (the outer one rejects
         * a fragment node). */
        let scope = if this.node().node_type() == NodeType::DocumentFragment {
            Scope::Inner
        } else {
            Scope::Outer
        };
        render(ruby, this.node(), scope, layout)
    })
}

/// Inner HTML: the node's children, without the node's own tag.
///
/// WHATWG special-cases a `<template>`: its inner HTML is its template contents,
/// a separate fragment rather than the element's (empty) children - the same
/// node `Element#content_fragment` exposes and `#to_html` serializes, so the
/// three agree.
fn inner_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let layout = layout_opt(ruby, args)?;
        let node = this.node();
        let target = node.template_content().unwrap_or(node);
        render(ruby, target, Scope::Inner, layout)
    })
}

/// From `Init_makiri`.
pub fn init_serialize() -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.defined()?;
    for name in ["to_html", "to_s", "outer_html"] {
        m.define_method(name, method!(to_html, -1))?;
    }
    m.define_method("inner_html", method!(inner_html, -1))?;
    Ok(())
}
