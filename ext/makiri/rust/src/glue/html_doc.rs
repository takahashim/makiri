//! `Makiri::HTML::Document` and the fragment entry points.
//!
//!   `Document._parse(source)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`, `#fragment(html, context:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:)`, `Node#parse(html)`
//!
//! The Document wrapper and its parsed handle, and the fragment pipeline, are
//! the bridge's ([`crate::bridge::wrapper`], [`crate::bridge::doc`],
//! [`crate::bridge::fragment`]); this module is the Ruby surface on them.

#![forbid(unsafe_code)]

use crate::bridge::fragment;
use crate::bridge::wrapper::keepalive_document;
use magnus::{method, prelude::*, Error, Ruby, Value};

/// The doctype node type, generated (see lexbor::abi).
const NODE_TYPE_DOCUMENT_TYPE: u32 = crate::lexbor::adapter::html::TYPE_DOCTYPE;

/* ---- Document.parse ---- */

fn doc_s_parse(_klass: Value, source: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::parse_document(source))
}

/* ---- read-only accessors ---- */

fn doc_root(ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_root(ruby, self_)))
}

fn doc_title(ruby: &Ruby, self_: Value) -> Result<magnus::RString, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_title(ruby, self_)))
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(_ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = crate::glue::html_node::arg_node(&self_)?;
        let doctype = doc
            .children()
            .find(|c| c.node_type() == NODE_TYPE_DOCUMENT_TYPE);
        Ok(crate::glue::html_node::wrap_node(doctype, self_))
    })
}

fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_quirks_mode(ruby, self_)))
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(_ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_errors(self_)))
}

/* ---- fragment entry points ---- */

/// `document.fragment(html, context: ...)` -> a DocumentFragment bound to this
/// document. `context` defaults to `<body>`.
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| fragment_from_args(ruby, args, || Ok(self_)))
}

/// `(html, context:)` from the argument list, parsed as a fragment bound to the
/// document `document` supplies - the one thing the two entry points differ in.
fn fragment_from_args(
    ruby: &Ruby,
    args: &[Value],
    document: impl FnOnce() -> Result<Value, Error>,
) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let context = a.keywords.get(ruby.sym_new("context"));
    let document = document()?;
    let at = fragment::resolve_fragment_context(document, context)?;
    fragment::build_fragment(document, a.required.0, at)
}

/// `DocumentFragment.parse(html, context: ...)` -> a standalone fragment with
/// its own backing document, kept alive by the fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| fragment_from_args(ruby, args, fragment::fragment_shell_document))
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(context) = crate::glue::html_node::arg_node(&self_)?.element() else {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "Node#parse requires an element context",
            ));
        };
        /* Only the context's tag and namespace ids are needed, read before the
         * fragment parse runs. */
        let at = fragment::FragmentTag::of(context.node());
        let document = keepalive_document(self_)?;
        let frag = fragment::build_fragment(document, rb_html, at)?;
        /* The native children reader, not a Ruby `children` dispatch: the
         * fragment is ours, and a subclass could redefine the method. */
        let frag = <crate::bridge::html::HtmlSelf as magnus::TryConvert>::try_convert(frag)?;
        crate::glue::html_node::read::children(ruby, frag)
    })
}

/// `Document#import_node(node, deep = false)` -> a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
fn doc_import_node(_ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
        /* `deep` is truthiness, anything but nil and false - the DOM default off. */
        let deep = a.optional.0.is_some_and(|v| v.to_bool());
        crate::bridge::doc::import_node(self_, a.required.0, deep)
    })
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
pub fn node_clone_node(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        /* The 0..1 arity by hand: `scan_args` cost about a third of a shallow
         * clone. The message is the one `rb_scan_args` gives. */
        let deep = match args {
            [] => false,
            /* Truthiness: anything but nil and false. */
            [v] => v.to_bool(),
            _ => {
                return Err(Error::new(
                    ruby.exception_arg_error(),
                    format!(
                        "wrong number of arguments (given {}, expected 0..1)",
                        args.len()
                    ),
                ))
            }
        };
        crate::bridge::doc::clone_node(rb_self, deep)
    })
}

/* ---- registration ---- */

/// The HTML Document surface. From `Init_makiri`, after the classes exist.
pub fn init_html_doc() {
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
}
