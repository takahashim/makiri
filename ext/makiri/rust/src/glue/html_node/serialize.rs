//! `Node#to_html` / `#to_s` / `#outer_html` and `#inner_html`.
//!
//! Reads the receiver and the `pretty:` option, picks the tree or deep
//! serializer, and copies the owned bytes into a String. The Lexbor callbacks
//! and the buffer live in [`crate::lexbor::serialize`], which does not know
//! about Ruby.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};

use super::HtmlSelf;
use crate::bridge::ruby::makiri_error;
use crate::init::MOD_HTML_NODE_METHODS;
use crate::lexbor::adapter::html::TYPE_FRAGMENT;
use crate::lexbor::serialize::serialize;

/// The optional `pretty:` keyword.
///
/// Read for truthiness rather than converted to `bool`: `pretty: nil` is false
/// and any other value - `0` included, this being Ruby - is true. Unknown
/// keywords are ignored, which is also why the hash is read with a plain lookup
/// rather than `get_kwargs`: that allocates a second hash to hold the keys it was
/// not asked about.
///
/// The no-argument call returns before any of that. It is the overwhelmingly
/// common one - `to_html` with a keyword is the exception - and routing it
/// through `scan_args` cost about a quarter of the per-call throughput on a
/// small element, which is all this method does at that size.
fn pretty_opt(ruby: &Ruby, args: &[Value]) -> Result<bool, Error> {
    if args.is_empty() {
        return Ok(false);
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    Ok(scanned
        .keywords
        .get(ruby.to_symbol("pretty"))
        .is_some_and(|v: Value| v.to_bool()))
}

/// The receiver's serialization as a UTF-8 String, or `Makiri::Error` on a
/// Lexbor status failure.
fn render(ruby: &Ruby, this: &HtmlSelf, deep: bool, pretty: bool) -> Result<RString, Error> {
    let buf = serialize(this.raw(), deep, pretty)
        .ok_or_else(|| makiri_error("HTML serialization failed"))?;
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
        let deep = this.node().node_type() == TYPE_FRAGMENT;
        render(ruby, &this, deep, pretty)
    })
}

/// Inner HTML: the node's children, without the node's own tag.
fn inner_html(ruby: &Ruby, this: HtmlSelf, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let pretty = pretty_opt(ruby, args)?;
        render(ruby, &this, true, pretty)
    })
}

/// From `Init_makiri`.
pub fn init_serialize() {
    let m = MOD_HTML_NODE_METHODS.module();
    for name in ["to_html", "to_s", "outer_html"] {
        m.define_method(name, method!(to_html, -1))
            .expect("defining an HTML serializer method");
    }
    m.define_method("inner_html", method!(inner_html, -1))
        .expect("defining an HTML serializer method");
}
