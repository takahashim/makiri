//! `Node#to_html` / `#to_s` / `#outer_html` and `#inner_html`.
//!
//! Reads the receiver and the `pretty:` option, picks the tree or deep
//! serializer, and copies the owned bytes into a String. The Lexbor callbacks
//! and the buffer live in [`crate::lexbor::serialize`], which does not know
//! about Ruby.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RString, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::ruby::makiri_error;
use crate::glue::kwargs::Kwargs;
use crate::init::MOD_HTML_NODE_METHODS;
use crate::lexbor::adapter::html::{NodeType, RawNode};
use crate::lexbor::serialize::serialize;

/// The optional `pretty:` keyword.
fn pretty_opt(ruby: &Ruby, args: &[Value]) -> Result<bool, Error> {
    Ok(Kwargs::scan(args)?.flag(ruby, "pretty"))
}

/// The receiver's serialization as a UTF-8 String, or `Makiri::Error` on a
/// Lexbor status failure.
fn render(ruby: &Ruby, node: RawNode, deep: bool, pretty: bool) -> Result<RString, Error> {
    let buf =
        serialize(node, deep, pretty).ok_or_else(|| makiri_error("HTML serialization failed"))?;
    /* Lexbor emits UTF-8, so the String is tagged UTF-8 rather than built as
     * binary and re-tagged. */
    Ok(ruby.enc_str_new(buf.as_slice(), ruby.utf8_encoding()))
}

/// Outer HTML: the node itself plus its descendants. `pretty: true` indents.
fn to_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let pretty = pretty_opt(ruby, args)?;
        /* A document fragment has no tag of its own, so its "outer" is its
         * children: the deep serializer is the right one (the tree serializer
         * rejects a fragment node). */
        let deep = this.node().node_type() == NodeType::DocumentFragment;
        render(ruby, this.raw(), deep, pretty)
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
        let pretty = pretty_opt(ruby, args)?;
        let target = this
            .node()
            .template_content()
            .map_or(this.raw(), RawNode::from);
        render(ruby, target, true, pretty)
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
