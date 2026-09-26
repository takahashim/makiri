//! `Makiri::HTML::Document` and the fragment entry points.
//!
//!   `Document._parse(source, max_tree_depth)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`,
//!   `#fragment(html, context:, max_tree_depth:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:, max_tree_depth:)`, `Node#parse(html)`
//!
//! The Document wrapper and its parsed handle, and the fragment pipeline, are
//! the bridge's ([`crate::bridge::wrapper`], [`crate::bridge::doc`],
//! [`crate::bridge::fragment`]); this module is the Ruby surface on them.

#![forbid(unsafe_code)]

use crate::bridge::fragment;
use crate::bridge::wrapper::keepalive_document;
use crate::lexbor::adapter::html::NodeType;
use magnus::{method, prelude::*, Error, Ruby, Value};

/* ---- Document.parse ---- */

/// `Document._parse(source, max_tree_depth)`. The Ruby `Document.parse` turns
/// the keyword into the second argument; `nil` is the default limit.
fn doc_s_parse(ruby: &Ruby, _klass: Value, source: Value, depth: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let limit = crate::glue::kwargs::max_tree_depth(ruby, Some(depth))?;
        crate::bridge::doc::parse_document(source, limit)
    })
}

/* ---- read-only accessors ---- */

fn doc_root(_ruby: &Ruby, self_: Value) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_root(self_)))
}

fn doc_title(ruby: &Ruby, self_: Value) -> Result<magnus::RString, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::doc::document_title(ruby, self_)))
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(_ruby: &Ruby, self_: Value) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = crate::glue::html_node::arg_node(&self_)?;
        let doctype = doc
            .children()
            .find(|c| c.node_type() == NodeType::DocumentType);
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

/// `document.fragment(html, context: ..., max_tree_depth: ...)` -> a
/// DocumentFragment bound to this document. `context` defaults to `<body>`,
/// `max_tree_depth` to 400 (see `glue::kwargs::max_tree_depth`).
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| fragment_from_args(ruby, args, || Ok(self_)))
}

/// `(html, context:, max_tree_depth:)` from the argument list, parsed as a
/// fragment bound to the document `document` supplies - the one thing the two
/// entry points differ in.
fn fragment_from_args(
    ruby: &Ruby,
    args: &[Value],
    document: impl FnOnce() -> Result<Value, Error>,
) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let context = a.keywords.get(ruby.sym_new("context"));
    /* Read before the backing document is made, so a bad value costs nothing. */
    let limit =
        crate::glue::kwargs::max_tree_depth(ruby, a.keywords.get(ruby.sym_new("max_tree_depth")))?;
    let document = document()?;
    let at = fragment::resolve_fragment_context(document, context)?;
    fragment::build_fragment(document, a.required.0, at, limit)
}

/// `DocumentFragment.parse(html, context: ..., max_tree_depth: ...)` -> a
/// standalone fragment with its own backing document, kept alive by the
/// fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| fragment_from_args(ruby, args, fragment::fragment_shell_document))
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        /* Only the context's tag and namespace ids are needed, read before the
         * fragment parse runs. */
        let Some(at) = crate::glue::html_node::arg_node(&self_)?
            .element()
            .and_then(fragment::FragmentTag::of)
        else {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "Node#parse requires an element context",
            ));
        };
        let document = keepalive_document(self_)?;
        /* No keyword here, as in Nokogiri: the default limit. */
        let frag = fragment::build_fragment(document, rb_html, at, fragment::DepthLimit::DEFAULT)?;
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
pub fn init_html_doc() -> Result<(), Error> {
    let html_doc = crate::init::CLASS_HTML_DOCUMENT.defined()?;

    html_doc.define_singleton_method("_parse", method!(doc_s_parse, 2))?;
    html_doc.define_method("root", method!(doc_root, 0))?;
    html_doc.define_method("title", method!(doc_title, 0))?;
    html_doc.define_method("errors", method!(doc_errors, 0))?;
    html_doc.define_method("internal_subset", method!(doc_internal_subset, 0))?;
    html_doc.define_method("quirks_mode", method!(doc_quirks_mode, 0))?;
    html_doc.define_method("fragment", method!(doc_fragment, -1))?;
    html_doc.define_method("import_node", method!(doc_import_node, -1))?;

    let frag = crate::init::CLASS_DOCUMENT_FRAGMENT.defined()?;
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))?;

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = crate::init::MOD_HTML_NODE_METHODS.defined()?;
    node_methods.define_method("parse", method!(node_parse, 1))?;
    Ok(())
}
