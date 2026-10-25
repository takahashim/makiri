//! `Makiri::HTML::Document` and the fragment entry points.
//!
//!   `Document._parse(source, max_tree_depth, max_amplification, encoding)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`,
//!   `#fragment(html, context:, max_tree_depth:, max_amplification:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:, max_tree_depth:, max_amplification:)`, `Node#parse(html)`
//!
//! The Document wrapper and its parsed handle, and the fragment pipeline, are
//! the bridge's ([`crate::bridge::wrapper`], [`crate::bridge::doc`],
//! [`crate::bridge::fragment`]); this module is the Ruby surface on them.

#![forbid(unsafe_code)]

use crate::bridge::fragment;
use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::bridge::string::GivenEncoding;
use crate::glue::html_node::HtmlSelf;
use crate::html_dom::html::NodeType;
use crate::method;

/* ---- Document.parse ---- */

/// `Document._parse(source, max_tree_depth, max_amplification, encoding)`.
/// The Ruby `Document.parse` turns the keywords into the arguments after
/// `source`; `nil` is a default limit, and no encoding given.
fn doc_s_parse(
    ruby: &Ruby,
    _klass: Value,
    source: Value,
    depth: Value,
    amplification: Value,
    encoding: Value,
) -> Result<Value, Error> {
    let limit = crate::glue::kwargs::parse_limits(ruby, Some(depth), Some(amplification))?;
    let given = encoding_arg(ruby, encoding)?;
    crate::bridge::doc::parse_document(ruby, source, limit, given)
}

/// The `encoding:` keyword, which `Document.parse` hands over as nil or a
/// String (an `Encoding` or Symbol by its `to_s`): a name Ruby knows, read
/// as a String tagged with that encoding is (`"US-ASCII"` is UTF-8,
/// `"BINARY"` is sniffed, `"UTF-32LE"` is transcoded), or an Encoding
/// Standard label (`"latin1"`). Anything else is `ArgumentError`: a document
/// is never read in an encoding the caller did not ask for.
fn encoding_arg(ruby: &Ruby, v: Value) -> Result<Option<GivenEncoding>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    /* A name: a NUL in it is the ArgumentError any other unknown name is. */
    let label = crate::bridge::string::ruby_verified_name(ruby, v, "encoding")?;
    let unknown = |label: &[u8]| {
        Error::new(
            crate::bridge::rb::Builtin::ArgumentError,
            format_args!(
                "unknown encoding for HTML: {} (omit encoding: to have it sniffed)",
                crate::msg::Lossy(label)
            ),
        )
    };
    /* No encoding's name is this long, Ruby's or the Standard's; a shorter
     * one is copied out, so no argument's copy is held across the lookup
     * (which can autoload an encoding). */
    let mut name = [0u8; 64];
    let Some(dst) = name.get_mut(..label.len()) else {
        return Err(unknown(label.as_bytes()));
    };
    dst.copy_from_slice(label.as_bytes());
    let len = label.len();
    drop(label);
    let name = &name[..len];
    GivenEncoding::resolve(ruby, name)
        .map(Some)
        .ok_or_else(|| unknown(name))
}

/// `Document#encoding`: the character encoding the document was read in, by
/// its Encoding Standard name (`"UTF-8"`, `"Shift_JIS"`, `"windows-1252"`).
fn doc_encoding(_ruby: &Ruby, this: Value) -> Result<&'static str, Error> {
    crate::bridge::doc::document_encoding(this)
}

/* ---- read-only accessors ---- */

fn doc_root(ruby: &Ruby, self_: Value) -> Result<Option<Value>, Error> {
    crate::bridge::doc::document_root(ruby, self_)
}

fn doc_title(ruby: &Ruby, self_: Value) -> Result<crate::bridge::rb::RString, Error> {
    crate::bridge::doc::document_title(ruby, self_)
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(ruby: &Ruby, this: HtmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |node| {
        node.children()
            .find(|c| c.node_type() == NodeType::DocumentType)
    })
}

/// `Makiri::HTML::Document.new`: an empty document - no children, no-quirks.
fn doc_s_new(ruby: &Ruby, _klass: Value) -> Result<Value, Error> {
    crate::bridge::doc::new_empty_document(ruby, 0, "UTF-8")
}

/// `Document#_empty_copy` (private): an empty document in this one's quirks
/// mode, for `#dup` to copy the children into.
fn doc_empty_copy(ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    let mode = u32::try_from(this.read()?.doc().compat_mode()).unwrap_or(0);
    let encoding = crate::bridge::doc::document_encoding(this.document)?;
    crate::bridge::doc::new_empty_document(ruby, mode, encoding)
}

fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::doc::document_quirks_mode(ruby, self_)
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(_ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    Ok(crate::bridge::doc::document_errors(self_))
}

/* ---- fragment entry points ---- */

/// `document.fragment(html, context: ..., max_tree_depth: ...,
/// max_amplification: ...)` -> a DocumentFragment bound to this document.
/// `context` defaults to `<body>`, the limits to `ParseLimits::DEFAULT` (see
/// `glue::kwargs::parse_limits`).
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    fragment_from_args(ruby, args, |_| Ok(self_))
}

/// `(html, context:, max_tree_depth:, max_amplification:)` from the argument list, parsed as a
/// fragment bound to the document `document` supplies - the one thing the two
/// entry points differ in.
fn fragment_from_args(
    ruby: &Ruby,
    args: &[Value],
    document: impl FnOnce(&Ruby) -> Result<Value, Error>,
) -> Result<Value, Error> {
    let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::with_keywords(1, 0))?;
    let kw = crate::glue::kwargs::Kwargs::from_scan(a.keywords());
    let context = kw.value(ruby, "context")?;
    /* Read before the backing document is made, so a bad value costs nothing. */
    let limit = crate::glue::kwargs::parse_limits(
        ruby,
        kw.value(ruby, "max_tree_depth")?,
        kw.value(ruby, "max_amplification")?,
    )?;
    let document = document(ruby)?;
    let at = fragment::resolve_fragment_context(ruby, document, context)?;
    fragment::build_fragment(ruby, document, a.required(0), at, limit)
}

/// `DocumentFragment.parse(html, context: ..., max_tree_depth: ...,
/// max_amplification: ...)` -> a
/// standalone fragment with its own backing document, kept alive by the
/// fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    fragment_from_args(ruby, args, fragment::fragment_shell_document)
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    if this.read()?.node().element().is_none() {
        return Err(Error::new(
            crate::bridge::rb::Builtin::ArgumentError,
            "Node#parse requires an element context",
        ));
    }
    /* The element itself is the context, read whole as `inner_html=`
     * reads it. No keyword here, as in Nokogiri: the default limit. */
    let at = fragment::ContextArg::element(ruby, this.value)?;
    let frag = fragment::build_fragment(
        ruby,
        this.document,
        rb_html,
        at,
        fragment::ParseLimits::DEFAULT,
    )?;
    /* The native children reader, not a Ruby `children` dispatch: the
     * fragment is ours, and a subclass could redefine the method. */
    let frag =
        <crate::bridge::html::HtmlSelf as crate::bridge::rb::TryConvert>::try_convert(ruby, frag)?;
    crate::glue::html_node::read::children(ruby, frag)
}

/// `Document#import_node(node, deep = false)` -> a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
fn doc_import_node(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::positional(1, 1))?;
    /* `deep` is truthiness, anything but nil and false - the DOM default off. */
    let deep = a.optional(0).is_some_and(|v| v.to_bool());
    crate::bridge::doc::import_node(ruby, self_, a.required(0), deep)
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
pub fn node_clone_node(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::positional(0, 1))?;
    /* Truthiness: anything but nil and false. */
    let deep = a.optional(0).is_some_and(|v| v.to_bool());
    crate::bridge::doc::clone_node(ruby, rb_self, deep)
}

/* ---- registration ---- */

/// The HTML Document surface. From `Init_makiri`, after the classes exist.
pub fn init_html_doc(ruby: &Ruby) -> Result<(), Error> {
    let html_doc = crate::init::CLASS_HTML_DOCUMENT.defined(ruby)?;

    html_doc.define_singleton_method("_parse", method!(doc_s_parse, 4))?;
    html_doc.define_method("encoding", method!(doc_encoding, 0))?;
    html_doc.define_singleton_method("new", method!(doc_s_new, 0))?;
    html_doc.define_private_method("_empty_copy", method!(doc_empty_copy, 0))?;
    html_doc.define_method("root", method!(doc_root, 0))?;
    html_doc.define_method("title", method!(doc_title, 0))?;
    html_doc.define_method("errors", method!(doc_errors, 0))?;
    html_doc.define_method("internal_subset", method!(doc_internal_subset, 0))?;
    html_doc.define_method("quirks_mode", method!(doc_quirks_mode, 0))?;
    html_doc.define_method("fragment", method!(doc_fragment, -1))?;
    html_doc.define_method("import_node", method!(doc_import_node, -1))?;
    html_doc.define_method(
        "tree_version",
        method!(crate::glue::node::document_tree_version, 0),
    )?;
    html_doc.define_method(
        "attribute_version",
        method!(crate::glue::node::document_attribute_version, 0),
    )?;

    let frag = crate::init::CLASS_DOCUMENT_FRAGMENT.defined(ruby)?;
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))?;

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = crate::init::MOD_HTML_NODE_METHODS.defined(ruby)?;
    node_methods.define_method("parse", method!(node_parse, 1))?;
    Ok(())
}
