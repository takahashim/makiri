//! `Makiri::XML::Document` and `Makiri::XML::DocumentFragment`: the native
//! parser, and the document-level readers and constructors.
//!
//!   `Document.parse(source, max_bytes:)` / `Makiri::XML(source)`,
//!   `Document.new`, `#root`, `#internal_subset`, `#fragment`,
//!   `DocumentFragment.parse`
//!
//! Nothing here touches Lexbor: the engine on the other side is Makiri's own XML
//! reader. The parse releases the GVL, and that - the strict decode, the copy of
//! the source taken before the release, the wrapper - is `bridge::xml`'s; here
//! only the arguments are read. Fragment parsing deliberately keeps the GVL: a
//! fragment is small, and an existing document's arena must never be changed
//! with the GVL down.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};
use crate::{function, method};

use crate::bridge::xml::wrap_xml_node as wrap;
use crate::init::{CLASS_XML_DOCUMENT, CLASS_XML_DOCUMENT_FRAGMENT};
use crate::xml::model::ParseLimits;

/// The optional per-parse budget overrides.
///
/// Only `max_bytes` is configurable today; an unknown keyword is an
/// `ArgumentError`, and new budgets join here as they become runtime-settable.
/// It must be a positive Integer - the sign is checked BEFORE the unsigned
/// conversion, because a negative would otherwise wrap into a huge `size_t` and
/// bypass the budget entirely.
fn parse_limits(ruby: &Ruby, h: Option<RHash>) -> Result<ParseLimits, Error> {
    let mut limits = ParseLimits { max_bytes: None };
    let Some(h) = h.filter(|h| !h.is_empty()) else {
        return Ok(limits);
    };

    let key = crate::bridge::build::symbol("max_bytes")?;
    /* The keys are read from the Hash itself (`hash::each_pair`), not
     * through a `keys` a subclass can redefine. */
    crate::glue::hash::each_pair(ruby, h, |k, _| {
        /* By identity: `eql?` would be the caller's key's method. */
        if crate::bridge::ruby::same_value(k, key.as_value()) {
            return Ok(());
        }
        Err(Error::new(
            ruby.exception_arg_error(),
            format!("unknown keyword: {}", crate::bridge::ruby::describe(k)),
        ))
    })?;

    /* `nil` is the default, as the documented `max_bytes: nil` says - and as
     * `max_tree_depth: nil` is on the HTML side. */
    let Some(v) = h.get(key).filter(|v: &Value| !v.is_nil()) else {
        return Ok(limits);
    };
    /* An actual Integer, not merely something convertible: a Float (1.5) is a
     * TypeError rather than a silent truncation. */
    let n = crate::bridge::rb::Integer::from_value(v)
        .ok_or_else(|| Error::new(ruby.exception_type_error(), "max_bytes must be an Integer"))?
        .to_i64()?;
    if n <= 0 {
        return Err(Error::new(
            ruby.exception_arg_error(),
            "max_bytes must be positive",
        ));
    }
    limits.max_bytes = Some(n as usize);
    Ok(limits)
}

/// `Makiri::XML::Document._parse(source, max_bytes: nil)`, behind `.parse`
/// (`lib/makiri/xml/document.rb`), which reads an IO and converts the source
/// to a String first, as the HTML entry does: `source` is a String here.
fn s_parse(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::with_keywords(1, 0))?;
    let source = a.required(0);
    let limits = parse_limits(ruby, a.keywords())?;
    crate::bridge::xml::parse_xml_document(source, limits)
}

fn doc_root(_ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    crate::bridge::xml::document_root(rb_self)
}

/// The document's DOCTYPE, or nil.
///
/// The name and the external/system identifiers are read; the DTD body is NOT
/// parsed - no entity or element declarations are loaded, so `&name;` stays an
/// undefined-entity error and no external subset is fetched. The node is kept
/// off the tree, so XPath never sees it (XPath 1.0 has no doctype node type).
fn doc_internal_subset(_ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    crate::bridge::xml::document_internal_subset(rb_self)
}

/// `Makiri::XML::Document.new` - an empty document to build up programmatically.
/// Any arguments (Nokogiri accepts a version and encoding) are accepted and
/// ignored.
fn document_s_new(_args: &[Value]) -> Result<Value, Error> {
    crate::bridge::xml::new_empty_xml_document()
}

/// `Makiri::XML::DocumentFragment.parse(source)` - a standalone fragment with
/// its own empty backing document. Self-contained: a prefixed name must declare
/// its namespace within the fragment itself. Use `Document#fragment` to parse
/// against an existing document's in-scope namespaces instead.
fn fragment_s_parse(_klass: Value, source: Value) -> Result<Value, Error> {
    let doc_obj = crate::bridge::xml::new_empty_xml_document()?;
    let frag = crate::bridge::xml::fragment_into(doc_obj, source, false)?;
    wrap(frag, doc_obj)
}

/// `Document#_copy` (private): the whole document, copied node for node - what
/// `#dup` returns.
fn doc_copy(rb_self: Value) -> Result<Value, Error> {
    crate::bridge::xml::copy_xml_document(rb_self)
}

/// `doc.fragment(source)` - a fragment bound to this document, resolving names
/// against its in-scope (root) namespaces, so the nodes can be spliced in.
fn doc_fragment(rb_self: Value, source: Value) -> Result<Value, Error> {
    let frag = crate::bridge::xml::fragment_into(rb_self, source, true)?;
    wrap(frag, rb_self)
}

/// The XML Document surface. From `Init_makiri`; the class itself is defined
/// with the rest of the hierarchy in `init`.
pub fn init_xml_doc() -> Result<(), Error> {
    let doc = CLASS_XML_DOCUMENT.defined()?;

    doc.define_singleton_method("_parse", function!(s_parse, -1))?;
    doc.define_singleton_method("new", function!(document_s_new, -1))?;
    doc.define_method("root", method!(doc_root, 0))?;
    doc.define_method("internal_subset", method!(doc_internal_subset, 0))?;
    doc.define_method("fragment", method!(doc_fragment, 1))?;
    doc.define_private_method("_copy", method!(doc_copy, 0))?;
    doc.define_method(
        "tree_version",
        method!(crate::glue::node::document_tree_version, 0),
    )?;
    doc.define_method(
        "attribute_version",
        method!(crate::glue::node::document_attribute_version, 0),
    )?;

    CLASS_XML_DOCUMENT_FRAGMENT
        .defined()?
        .define_singleton_method("parse", method!(fragment_s_parse, 1))?;
    Ok(())
}
