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
use crate::bridge::string::GivenEncoding;
use crate::glue::html_node::HtmlSelf;
use crate::html_dom::html::NodeType;
use magnus::{method, prelude::*, Error, Ruby, Value};

/* ---- Document.parse ---- */

/// `Document._parse(source, max_tree_depth, encoding)`. The Ruby
/// `Document.parse` turns the keywords into the second and third arguments;
/// `nil` is the default limit, and no encoding given.
fn doc_s_parse(
    ruby: &Ruby,
    _klass: Value,
    source: Value,
    depth: Value,
    encoding: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let limit = crate::glue::kwargs::max_tree_depth(ruby, Some(depth))?;
        let given = encoding_arg(ruby, encoding)?;
        crate::bridge::doc::parse_document(source, limit, given)
    })
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
    let label = crate::bridge::string::ruby_verified_name(v, "encoding")?;
    let unknown = |label: &str| {
        Error::new(
            ruby.exception_arg_error(),
            format!("unknown encoding for HTML: {label} (omit encoding: to have it sniffed)"),
        )
    };
    /* No encoding's name is this long, Ruby's or the Standard's; a shorter
     * one is copied out, so no argument's copy is held across the lookup
     * (which can autoload an encoding). */
    let mut name = [0u8; 64];
    let Some(dst) = name.get_mut(..label.len()) else {
        return Err(unknown(&label));
    };
    dst.copy_from_slice(label.as_bytes());
    let len = label.len();
    drop(label);
    let name = &name[..len];
    GivenEncoding::resolve(name)
        .map(Some)
        .ok_or_else(|| unknown(&String::from_utf8_lossy(name)))
}

/// `Document#encoding`: the character encoding the document was read in, by
/// its Encoding Standard name (`"UTF-8"`, `"Shift_JIS"`, `"windows-1252"`).
fn doc_encoding(_ruby: &Ruby, this: Value) -> Result<&'static str, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::document_encoding(this))
}

/* ---- read-only accessors ---- */

fn doc_root(_ruby: &Ruby, self_: Value) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::document_root(self_))
}

fn doc_title(_ruby: &Ruby, self_: Value) -> Result<magnus::RString, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::document_title(self_))
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(_ruby: &Ruby, this: HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        this.read()?.wrap(|node| {
            node.children()
                .find(|c| c.node_type() == NodeType::DocumentType)
        })
    })
}

/// `Makiri::HTML::Document.new`: an empty document - no children, no-quirks.
fn doc_s_new(_ruby: &Ruby, _klass: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::new_empty_document(0, "UTF-8"))
}

/// `Document#_empty_copy` (private): an empty document in this one's quirks
/// mode, for `#dup` to copy the children into.
fn doc_empty_copy(_ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let mode = u32::try_from(this.read()?.doc().compat_mode()).unwrap_or(0);
        let encoding = crate::bridge::doc::document_encoding(this.document)?;
        crate::bridge::doc::new_empty_document(mode, encoding)
    })
}

fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| crate::bridge::doc::document_quirks_mode(ruby, self_))
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
fn node_parse(ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        if this.read()?.node().element().is_none() {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "Node#parse requires an element context",
            ));
        }
        /* The element itself is the context, read whole as `inner_html=`
         * reads it. No keyword here, as in Nokogiri: the default limit. */
        let at = fragment::ContextArg::element(this.value)?;
        let frag =
            fragment::build_fragment(this.document, rb_html, at, fragment::DepthLimit::DEFAULT)?;
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

    html_doc.define_singleton_method("_parse", method!(doc_s_parse, 3))?;
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

    let frag = crate::init::CLASS_DOCUMENT_FRAGMENT.defined()?;
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))?;

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = crate::init::MOD_HTML_NODE_METHODS.defined()?;
    node_methods.define_method("parse", method!(node_parse, 1))?;
    Ok(())
}
