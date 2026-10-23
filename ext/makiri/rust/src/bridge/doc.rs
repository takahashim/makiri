//! The HTML Document: parsing one, its read-only accessors, and import/clone.
//! Fragments are `bridge::fragment`'s; the wrapper and its mutation gate are
//! `bridge::wrapper`'s.
//!
//! The arena work is `html`'s; what lives here is the part that must touch
//! raw Ruby values and arena handles together - copying the source out of a
//! Ruby String before the wrapper allocation, releasing the GVL for the parse,
//! and minting the wrapper. Keeping it here is what lets `glue/html_doc.rs` be
//! ordinary safe orchestration.
//!
//! # Parsing releases the GVL
//!
//! `parse_html` and everything under it is Ruby-free, and a freshly parsed
//! document is not yet shared, so nothing can race it. The source is copied into
//! a C buffer BEFORE the wrapper is allocated: allocating the wrapper is a GC
//! point, and the copy must not straddle one while holding a borrowed pointer
//! into a Ruby String's backing store.

#![forbid(unsafe_code)]

use magnus::{Error, RString, Ruby, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::html::new_node;
use crate::bridge::html::{html_node_key, with_html_node, wrap_html_node};
use crate::bridge::string::DocumentSource;
use crate::bridge::wrapper::{
    html_read, node_repr, xml_read, DocKind, DocumentShell, NodeRepr, DOC_TYPE,
};
use crate::bridge::xml::xml_node_document;
use crate::bridge::xml::{xml_mut_result, xml_node_unwrap};
use crate::html_dom::html::RawNode;
use crate::html_dom::post_parse::{empty_html_document, HtmlParseError};
use crate::html_dom::tree_guard::{DepthLimit, GuardStop};

/* ------------------------------------------------------------------ *
 * parsing                                                            *
 * ------------------------------------------------------------------ */

/// The error for a parse the guard refused (`GuardStop`), for a document and
/// a fragment alike: the one place that maps a stop to its exception.
pub fn guard_error(stop: GuardStop, limit: DepthLimit) -> Error {
    match stop {
        GuardStop::TooDeep => tree_depth_error(limit),
    }
}

/// The error for a parse the tree-depth limit refused: `Makiri::Error`, naming
/// the limit, for a document and a fragment alike.
pub fn tree_depth_error(limit: DepthLimit) -> Error {
    match limit.max_depth() {
        Some(n) => makiri_error(format!("document tree depth limit exceeded ({n})")),
        /* An unlimited parse is never refused for depth. */
        None => crate::bridge::ruby::internal_error(
            "a parse with no tree-depth limit was refused for depth",
        ),
    }
}

/// `Document._parse(source, max_tree_depth)`: parse HTML, releasing the GVL,
/// and wrap it.
///
/// The Ruby-level `Document.parse` converts `source` to a String (and reads IO)
/// before calling this. Source locations for `Node#line` are always tracked.
/// A tree deeper than `limit` fails the parse (see `html_dom::tree_guard`).
pub fn parse_document(
    source: Value,
    limit: DepthLimit,
    given: Option<crate::bridge::string::GivenEncoding>,
) -> Result<Value, Error> {
    let s = crate::bridge::ruby::string_arg(source)?;
    /* Allocate the wrapper with a null handle, so a failed parse still
     * frees cleanly through GC - and first, before this frame owns the
     * source's copy: the wrapper's allocation can raise. This entry is
     * defined on Makiri::HTML::Document, so the result is always HTML. */
    let shell = DocumentShell::new(DocKind::Html)?;
    /* The bytes as they are, with the transport layer's encoding: `given`,
     * else the String's own (none for binary, which is sniffed). The copy is
     * what the parse reads with the GVL released, so nothing here borrows
     * from the Ruby String. */
    let src = DocumentSource::from_ruby(s, given)?;

    /* The body - sniffing, decoding and parsing - runs on this thread with
     * the GVL released, so the parsed handle comes back as it is. */
    let result = crate::bridge::gvl::without_gvl(|| {
        crate::html::tree::parse_document_bytes(src.input(), limit)
    })?;
    drop(src);

    let parsed = result.map_err(|e| match e {
        HtmlParseError::Guard(stop) => guard_error(stop, limit),
        HtmlParseError::Failed => makiri_error("failed to parse HTML document"),
    })?;
    /* The GC learns the arena's size in `install`; `owned` is already gone, so
     * a collection that triggers has nothing of ours to invalidate. */
    Ok(shell.install_html(parsed))
}

/// An empty HTML document - no children - in `compat_mode` (Lexbor's
/// numbering) and `encoding`: `Makiri::HTML::Document.new` (no-quirks,
/// UTF-8) and the copy `Document#dup` fills.
pub fn new_empty_document(compat_mode: u32, encoding: &'static str) -> Result<Value, Error> {
    /* The wrapper first, while nothing needs freeing - see DocumentShell. */
    let shell = DocumentShell::new(DocKind::Html)?;
    let mut parsed = empty_html_document(compat_mode)
        .map_err(|_| makiri_error("failed to create HTML document"))?;
    parsed.set_encoding(encoding);
    Ok(shell.install_html(parsed))
}

/* ------------------------------------------------------------------ *
 * read-only accessors                                                *
 * ------------------------------------------------------------------ */

/// `Document#root`: the root Element node, or nil - for `Document.new`, or once
/// the root is removed (the HTML parser inserts html/head/body even for empty
/// input).
pub fn document_root(rb_doc: Value) -> Result<Option<Value>, Error> {
    let root = {
        let read = html_read(&rb_doc)?;
        read.doc().as_node().document_root().map(RawNode::from)
    };
    root.map(|root| wrap_html_node(root, rb_doc)).transpose()
}

/// `Document#title`: the document `<title>`, or `""`.
///
/// Protected: a collapsed title is this frame's own copy, which the String's
/// allocation raising past the frame would leak.
pub fn document_title(rb_doc: Value) -> Result<RString, Error> {
    let read = html_read(&rb_doc)?;
    let title = read.doc().title();
    let bytes = title.as_deref().unwrap_or(&[]);
    crate::bridge::string::utf8_rstring(bytes)
}

/// `Document#encoding`: the character encoding the document was read in, by
/// its Encoding Standard name - `"UTF-8"` for one made rather than parsed.
pub fn document_encoding(rb_doc: Value) -> Result<&'static str, Error> {
    crate::bridge::wrapper::with_html_parsed_known(rb_doc, |p| p.encoding())
}

/// `Document#quirks_mode`, as an Integer matching Lexbor (and Gumbo/Nokogiri):
/// 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the parser from the doctype.
pub fn document_quirks_mode(_ruby: &Ruby, rb_doc: Value) -> Result<Value, Error> {
    let mode = html_read(&rb_doc)?.doc().compat_mode();
    crate::bridge::build::int(mode)
}

/// `Document#errors`: the (currently always empty) parse-warning Array.
pub fn document_errors(rb_doc: Value) -> Value {
    /* A Document method, so the receiver is a Document. */
    DOC_TYPE.get_known(&rb_doc).errors()
}

/* ------------------------------------------------------------------ *
 * import and clone                                                   *
 * ------------------------------------------------------------------ */

/// `Document#import_node(node, deep = false)`: a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
///
/// Unlike `Node#clone_node` the copy belongs to the receiver, so this is the way
/// to bring a node across documents (Makiri never moves one between arenas). The
/// source is untouched and the copy is detached.
pub fn import_node(rb_self: Value, node_v: Value, deep: bool) -> Result<Value, Error> {
    crate::bridge::html::check_owning_doc(&rb_self)?;

    /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
     * detached lxb subtree owned by this document. */
    if node_repr(node_v) == NodeRepr::Xml {
        let xml_doc = xml_node_document(node_v)?;
        let src = xml_node_unwrap(node_v)?;
        let imp = {
            let xdoc = xml_read(&xml_doc)?;
            new_node(&rb_self, |d| d.import_xml(&xdoc, src, deep))?
        };
        return wrap_html_node(xml_mut_result(imp)?, rb_self);
    }

    /* Err on a non-node. A node of THIS document is copied by reference -
     * the edit holds the document, and reads it through its own borrow; one
     * of another document is read through that document's. */
    let (key, src_doc) = html_node_key(node_v)?;
    /* The DOM's importNode refuses a document (NotSupportedError), as
     * `clone_node` does here. */
    if crate::bridge::ruby::same_value(node_v, src_doc) {
        return Err(makiri_error(
            "import_node cannot import a document; use #dup",
        ));
    }
    let raw = crate::bridge::html::node_ref(src_doc, key)?;
    let imp = if crate::bridge::ruby::same_value(src_doc, rb_self) {
        new_node(&rb_self, |d| d.copy(raw, deep))?
    } else {
        with_html_node(src_doc, key, |src| {
            new_node(&rb_self, |d| d.import(src, deep))
        })??
    };
    let imp = imp.map_err(|_| makiri_error("failed to import node"))?;
    wrap_html_node(imp, rb_self)
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
///
/// Built on the same import + `<template>`-content fixup as the fragment parser,
/// so a deep-cloned `<template>` carries its contents (which `import_node` alone
/// omits). Fails closed: a null import is an error rather than a partial node.
pub fn clone_node(rb_self: Value, deep: bool) -> Result<Value, Error> {
    let (key, document) = html_node_key(rb_self)?;
    /* A Document's copy would wrap back to the receiver itself - the original
     * handed out as a copy. `HTML::Document#dup` is the document copy. */
    if crate::bridge::ruby::same_value(rb_self, document) {
        return Err(makiri_error("clone_node cannot copy a document; use #dup"));
    }
    let raw = crate::bridge::html::node_ref(document, key)?;
    let clone = new_node(&document, |d| d.copy(raw, deep))?
        .map_err(|_| makiri_error("failed to clone node"))?;
    wrap_html_node(clone, document)
}
