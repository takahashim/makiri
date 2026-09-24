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

use magnus::{function, method, prelude::*, Error, RArray, RClass, RHash, Ruby, Value};

use crate::bridge::xml::wrap;
use crate::init::{CLASS_XML_DOCUMENT, CLASS_XML_DOCUMENT_FRAGMENT};
use crate::xml::model::{Limits as XmlLimits, MAX_BYTES};

/// The optional per-parse budget overrides.
///
/// Only `max_bytes` is configurable today; an unknown keyword is an
/// `ArgumentError`, and new budgets join here as they become runtime-settable.
/// It must be a positive Integer - the sign is checked BEFORE the unsigned
/// conversion, because a negative would otherwise wrap into a huge `size_t` and
/// bypass the budget entirely.
fn parse_limits(ruby: &Ruby, h: RHash) -> Result<XmlLimits, Error> {
    let mut limits = XmlLimits { max_bytes: 0 };
    if h.is_empty() {
        return Ok(limits);
    }

    let key = ruby.sym_new("max_bytes");
    let keys: RArray = h.funcall("keys", ())?;
    for k in keys.into_iter() {
        if !k.eql(key)? {
            return Err(Error::new(
                ruby.exception_arg_error(),
                format!("unknown keyword: {}", k.inspect()),
            ));
        }
    }

    let Some(v) = h.get(key) else {
        return Ok(limits);
    };
    /* An actual Integer, not merely something convertible: a Float (1.5) is a
     * TypeError rather than a silent truncation. */
    let n = magnus::Integer::from_value(v)
        .ok_or_else(|| Error::new(ruby.exception_type_error(), "max_bytes must be an Integer"))?
        .to_i64()?;
    if n <= 0 {
        return Err(Error::new(
            ruby.exception_arg_error(),
            "max_bytes must be positive",
        ));
    }
    limits.max_bytes = n as usize;
    Ok(limits)
}

/// `Makiri::XML::Document.parse(source, max_bytes: nil)`.
///
/// `source` is a String or anything answering `#read` (an IO / File / StringIO).
/// Read a non-UTF-8 file in binary mode so the encoding is autodetected from its
/// BOM or declaration.
fn s_parse(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let scanned = magnus::scan_args::scan_args::<(Value,), (), (), (), RHash, ()>(args)?;
        let (source,) = scanned.required;
        let limits = parse_limits(ruby, scanned.keywords)?;
        let budget = if limits.max_bytes != 0 {
            limits.max_bytes
        } else {
            MAX_BYTES
        };

        /* An IO/File-like source is read first, as the HTML entry does; a String
         * passes straight through. */
        let source = if source.respond_to("read", true)? {
            source.funcall::<_, _, Value>("read", ())?
        } else {
            source
        };
        crate::bridge::xml::parse_xml_document(source, limits, budget)
    })
}

fn doc_root(ruby: &Ruby, rb_self: Value) -> Value {
    crate::bridge::xml::document_root(ruby, rb_self)
}

/// The document's DOCTYPE, or nil.
///
/// The name and the external/system identifiers are read; the DTD body is NOT
/// parsed - no entity or element declarations are loaded, so `&name;` stays an
/// undefined-entity error and no external subset is fetched. The node is kept
/// off the tree, so XPath never sees it (XPath 1.0 has no doctype node type).
fn doc_internal_subset(ruby: &Ruby, rb_self: Value) -> Value {
    crate::bridge::xml::document_internal_subset(ruby, rb_self)
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
    crate::bridge::ruby::entry(|| {
        let doc_obj = crate::bridge::xml::new_empty_xml_document()?;
        let frag = crate::bridge::xml::fragment_into(doc_obj, source, false)?;
        Ok(wrap(frag, doc_obj))
    })
}

/// `doc.fragment(source)` - a fragment bound to this document, resolving names
/// against its in-scope (root) namespaces, so the nodes can be spliced in.
fn doc_fragment(rb_self: Value, source: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let frag = crate::bridge::xml::fragment_into(rb_self, source, true)?;
        Ok(wrap(frag, rb_self))
    })
}

/// The XML Document surface. From `Init_makiri`; the class itself is defined
/// with the rest of the hierarchy in `init`.
pub fn init_xml_doc() {
    let doc = RClass::from_value(CLASS_XML_DOCUMENT.value()).expect("Makiri::XML::Document");

    doc.define_singleton_method("parse", function!(s_parse, -1))
        .expect("Document.parse");
    doc.define_singleton_method("new", function!(document_s_new, -1))
        .expect("Document.new");
    doc.define_method("root", method!(doc_root, 0))
        .expect("#root");
    doc.define_method("internal_subset", method!(doc_internal_subset, 0))
        .expect("#internal_subset");
    doc.define_method("fragment", method!(doc_fragment, 1))
        .expect("#fragment");

    RClass::from_value(CLASS_XML_DOCUMENT_FRAGMENT.value())
        .expect("Makiri::XML::DocumentFragment")
        .define_singleton_method("parse", method!(fragment_s_parse, 1))
        .expect("DocumentFragment.parse");
}
