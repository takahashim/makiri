//! `Node#to_html` / `#to_s` / `#outer_html` and `#inner_html`
//! (glue/ruby_html_serialize.c).
//!
//! The Ruby-facing half of HTML serialization: it reads the receiver, picks the
//! tree or deep serializer, and copies the owned bytes into a String. The
//! Lexbor callbacks and the buffer live in [`crate::lexbor::serialize`], which
//! does not know about Ruby.

#![allow(unsafe_code)]

use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};

use crate::bridge::html::html_node_unwrap;
use crate::bridge::ruby::error_class;
use crate::init::MOD_HTML_NODE_METHODS;
use crate::lexbor::adapter::html::{RawNode, TYPE_FRAGMENT};
use crate::lexbor::serialize::serialize;

/// The optional `pretty:` keyword.
///
/// Read for truthiness rather than converted to `bool`, which is what
/// `RTEST(rb_hash_aref(opts, :pretty))` did: `pretty: nil` is false and any
/// other value - `0` included, this being Ruby - is true. Unknown keywords are
/// ignored, as the C's `rb_scan_args(argc, argv, "0:", ...)` did, which is also
/// why the hash is read with a plain lookup rather than `get_kwargs`: that
/// allocates a second hash to hold the keys it was not asked about.
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

/// The node's serialization as a UTF-8 String, or `Makiri::Error` on a Lexbor
/// status failure.
fn render(ruby: &Ruby, node: RawNode, deep: bool, pretty: bool) -> Result<RString, Error> {
    let buf = serialize(node, deep, pretty)
        .ok_or_else(|| Error::new(error_class(), "HTML serialization failed"))?;
    // Lexbor emits UTF-8, so the String is tagged UTF-8 rather than built as
    // binary and re-tagged (which is what str_from_slice would give).
    Ok(ruby.enc_str_new(buf.as_slice(), ruby.utf8_encoding()))
}

/// Outer HTML: the node itself plus its descendants. `pretty: true` indents.
fn to_html(rb_self: Value, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let ruby = Ruby::get_with(rb_self);
        let pretty = pretty_opt(&ruby, args)?;
        // The raising accessor, called while nothing is live (see the module docs).
        let node = html_node_unwrap(rb_self)?;

        // A document fragment has no tag of its own, so its "outer" is its
        // children: the deep serializer is the right one (the tree serializer
        // rejects a fragment node).
        /* SAFETY: the node of a live wrapper, which keeps its document alive. */
        let deep = unsafe { node.as_node() }.node_type() == TYPE_FRAGMENT;
        render(&ruby, node, deep, pretty)
    })
}

/// Inner HTML: the node's children, without the node's own tag.
fn inner_html(rb_self: Value, args: &[Value]) -> Result<RString, Error> {
    crate::bridge::ruby::entry(|| {
        let ruby = Ruby::get_with(rb_self);
        let pretty = pretty_opt(&ruby, args)?;
        render(&ruby, html_node_unwrap(rb_self)?, true, pretty)
    })
}

/// `init_serialize` - the same entry point Init_makiri already calls.
///
/// # Safety
/// Called from `Init_makiri`, on the Ruby thread with the GVL held, after the
/// classes and modules exist.
pub fn init_serialize() {
    let m = MOD_HTML_NODE_METHODS.module();
    for name in ["to_html", "to_s", "outer_html"] {
        m.define_method(name, method!(to_html, -1))
            .expect("defining an HTML serializer method");
    }
    m.define_method("inner_html", method!(inner_html, -1))
        .expect("defining an HTML serializer method");
}
