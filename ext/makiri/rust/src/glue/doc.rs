//! `Makiri::HTML::Document` and the fragment machinery (glue/ruby_doc.c).
//!
//!   `Document._parse(source)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`, `#fragment(html, context:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:)`, `Node#parse(html)`
//!
//! # The Document wrapper, and what other glue modules share
//!
//! The Document wrapper type and its `rb_data_type_t` chain, the parsed-handle
//! accessors, and the fragment pipeline live in the bridge
//! ([`crate::bridge::lexbor`], [`crate::bridge::doc`]); this module keeps the
//! Ruby methods, the evaluation guard, and the re-exports its callers already
//! name here.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};
pub use crate::bridge::lexbor::{
    doc_parsed, doc_parsed_known, html_doc_known, html_doc_unwrap, keepalive_document,
};

pub use crate::bridge::doc::{
    document_errors, document_quirks_mode, document_root, document_title, fragment_in,
    fragment_shell_document, import_node, parse_document,
};
pub use crate::lexbor::adapter::cross_import::cross_xml_to_html;
pub use crate::lexbor::adapter::post_parse::parse_html;
pub use crate::glue::node::node_kind;
pub use crate::glue::xml_node::mutate::xml_mut_check;
pub use crate::xml::api::xml_doc_memsize;

/// The doctype node type, generated (see lexbor_abi).
const NODE_TYPE_DOCUMENT_TYPE: u32 = crate::lexbor::adapter::html::TYPE_DOCTYPE;

/* ---- Document.parse ---- */

fn doc_s_parse(ruby: &Ruby, _klass: Value, source: Value) -> Result<Value, Error> {
    let _ = ruby;
    crate::bridge::doc::parse_document(source)
}

/* ---- read-only accessors ---- */

fn doc_root(ruby: &Ruby, self_: Value) -> Value {
    crate::bridge::doc::document_root(ruby, self_)
}

fn doc_title(ruby: &Ruby, self_: Value) -> magnus::RString {
    crate::bridge::doc::document_title(ruby, self_)
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(_ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    let doc = crate::glue::html_node::arg_node(&self_)?;
    let doctype = doc
        .children()
        .find(|c| c.node_type() == NODE_TYPE_DOCUMENT_TYPE);
    Ok(crate::glue::html_node::wrap_node(doctype, self_))
}

fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Value {
    crate::bridge::doc::document_quirks_mode(ruby, self_)
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(_ruby: &Ruby, self_: Value) -> Value {
    crate::bridge::doc::document_errors(self_)
}

/* ---- fragment entry points ---- */

/// `document.fragment(html, context: ...)` -> a DocumentFragment bound to this
/// document. `context` defaults to `<body>`.
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::doc::fragment_in(ruby, args, || Ok(self_))
}

/// `DocumentFragment.parse(html, context: ...)` -> a standalone fragment with
/// its own backing document, kept alive by the fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::doc::fragment_in(ruby, args, crate::bridge::doc::fragment_shell_document)
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    let Some(context) = crate::glue::html_node::arg_node(&self_)?.element() else {
        return Err(Error::new(
            ruby.exception_arg_error(),
            "Node#parse requires an element context",
        ));
    };
    /* Only the context's tag and namespace ids are needed, read before the
     * fragment parse runs. */
    let (tag, ns) = (context.node().tag_id(), context.node().ns_id());
    let document = keepalive_document(self_)?;
    let frag = crate::bridge::doc::build_fragment(ruby, document, rb_html, tag, ns)?;
    frag.funcall("children", ())
}

/// `Document#import_node(node, deep = false)` -> a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
fn doc_import_node(_ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::doc::import_node(self_, args)
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
pub fn node_clone_node(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::doc::clone_node(rb_self, args)
}

/* ---- registration ---- */

/// `Init_makiri` calls this where it called the C one.
///
/// # Safety
/// Runs once, from `Init_makiri`, on the Ruby thread.
pub fn init_document() {
    let ruby = Ruby::get().expect("init_document runs on the Ruby thread");
    let html_doc = magnus::RClass::from_value(crate::init::CLASS_HTML_DOCUMENT.value())
        .expect("Makiri::HTML::Document is a class");

    html_doc
        .define_singleton_method("_parse", method!(doc_s_parse, 1))
        .expect("Document._parse");
    html_doc
        .define_method("root", method!(doc_root, 0))
        .expect("Document#root");
    html_doc
        .define_method("title", method!(doc_title, 0))
        .expect("Document#title");
    html_doc
        .define_method("errors", method!(doc_errors, 0))
        .expect("Document#errors");
    html_doc
        .define_method("internal_subset", method!(doc_internal_subset, 0))
        .expect("Document#internal_subset");
    html_doc
        .define_method("quirks_mode", method!(doc_quirks_mode, 0))
        .expect("Document#quirks_mode");
    html_doc
        .define_method("fragment", method!(doc_fragment, -1))
        .expect("Document#fragment");
    html_doc
        .define_method("import_node", method!(doc_import_node, -1))
        .expect("Document#import_node");

    let frag = magnus::RClass::from_value(crate::init::CLASS_DOCUMENT_FRAGMENT.value())
        .expect("Makiri::DocumentFragment is a class");
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))
        .expect("DocumentFragment.parse");

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = magnus::RModule::from_value(crate::init::MOD_HTML_NODE_METHODS.value())
        .expect("Makiri::HTML::NodeMethods is a module");
    node_methods
        .define_method("parse", method!(node_parse, 1))
        .expect("Node#parse");

    let _ = ruby;
}
