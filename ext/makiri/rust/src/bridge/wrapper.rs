//! The Ruby wrappers of Makiri's nodes and Documents: the structs behind
//! them, their TypedData types and GC hooks, the one way a Document gets its
//! parsed handle ([`DocumentShell`]), and the representation-agnostic
//! accessors every node method starts from.
//!
//! Which representation a wrapper holds is decided here, by its TypedData
//! type; the HTML and XML front doors built on it are `bridge::html` and
//! `bridge::xml`.

#![allow(unsafe_code)]

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{prelude::*, Error, Value};

use crate::bridge::ruby::{value, VALUE};
use crate::bridge::typed::{Hooks, Marker, TypedType};
use crate::init::CLASS_DOCUMENT;
use crate::lexbor::adapter::html::{HtmlDoc, RawDoc};
use crate::lexbor::adapter::post_parse::Parsed;
use crate::xml::model::Doc as XmlDoc;

/* ------------------------------------------------------------------ *
 * the node wrapper                                                   *
 * ------------------------------------------------------------------ */

/// A node wrapper's data: the node pointer plus the keepalive Document.
///
/// The node is owned by the document's arena (HTML or XML), so the wrapper
/// never frees it; the Document reference is what keeps it alive, and marking
/// it is the wrapper's whole GC job.
pub struct NodeData {
    /// Representation-opaque; read it only through a kind-checked accessor.
    pub node: *mut c_void,
    pub document: VALUE,
}

impl Hooks for NodeData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.document);
    }
}

pub static NODE_DATA_TYPE: TypedType<NodeData> = TypedType::base(c"Makiri::Node".as_ptr());

pub static HTML_NODE_TYPE: TypedType<NodeData> =
    TypedType::derived(c"Makiri::HTML::Node".as_ptr(), &NODE_DATA_TYPE);

pub static XML_NODE_TYPE: TypedType<NodeData> =
    TypedType::derived(c"Makiri::XML::Node".as_ptr(), &NODE_DATA_TYPE);

/// Which representation a wrapped Ruby node is, decided by its TypedData type
/// rather than its Ruby class. A Document, a NodeSet or any non-node is
/// `Other`.
///
/// An enum, not the C's `c_int` codes: a transcribed code (`NODE_KIND_XML = 1`
/// where it was 2) once made `Document#import_node` read every HTML node as an
/// XML one, and copies of those numbers had spread to three files. A `match`
/// over this is checked for exhaustiveness and cannot be off by one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeRepr {
    Html,
    Xml,
    Other,
}

/* ------------------------------------------------------------------ *
 * the document wrapper                                               *
 * ------------------------------------------------------------------ */

/// A Document wrapper's data: the parsed handle (owned - GC frees it) and the reserved
/// errors Array.
pub struct DocData {
    /// Set once, by `DocumentShell::install`; read through the accessors below.
    parsed: *mut Parsed,
    errors: VALUE,
    /// The external bytes this wrapper has told the GC about, so `release`
    /// takes back exactly what [`account_document`] reported.
    reported: usize,
}

impl DocData {
    /// The Document's parse-warning Array.
    pub fn errors(&self) -> Value {
        // SAFETY: the live Array this wrapper marks.
        unsafe { value(self.errors) }
    }

    /// The bytes the handle holds outside Ruby's allocator, or 0 with none.
    fn external_bytes(&self) -> usize {
        // SAFETY: `parsed` is owned by this object and live for the call.
        unsafe { self.parsed.as_ref() }.map_or(0, Parsed::external_bytes)
    }
}

impl Hooks for DocData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.errors);
    }

    fn memsize(&self) -> usize {
        core::mem::size_of::<DocData>().saturating_add(self.external_bytes())
    }

    fn release(&mut self) {
        if !self.parsed.is_null() {
            // SAFETY: `parsed` came from `Box::into_raw` and only this owns it.
            unsafe { drop(Box::from_raw(self.parsed)) };
        }
        /* Balance the report, or the GC keeps counting freed arenas as live
         * and collects ever more eagerly. A plain C call, as this hook has to
         * be: it only subtracts, and Ruby's own `xfree` does the same from
         * here. */
        if let Ok(diff) = isize::try_from(self.reported) {
            // SAFETY: called from Ruby's free hook, with the GVL held.
            unsafe { rb_sys::rb_gc_adjust_memory_usage(diff.wrapping_neg() as rb_sys::ssize_t) };
        }
    }
}

/// Tell the GC how much memory `rb_doc` holds outside Ruby's allocator.
///
/// Neither the Lexbor arena nor the XML arena is an `xmalloc`, so the GC sees
/// a parsed document as a few dozen bytes: a loop that parses and drops never
/// triggers a collection from memory pressure, RSS climbs by a document per
/// parse, and every parse pays for freshly faulted pages (measured: 2× the
/// parse time, and gigabytes of RSS, on a 280 KB document). This reports the
/// difference since the last call, so it is safe to call again after the
/// document grows.
///
/// May run a collection right here, so `rb_doc` must be reachable from the
/// caller's frame (a local VALUE is), and nothing borrowed from a Ruby String
/// may be held across the call.
fn account_document(rb_doc: VALUE) {
    // SAFETY: `rb_doc` is a Document (the base type matches either leaf).
    let d = unsafe { &mut *(DOC_TYPE.known_ptr(value(rb_doc))) };
    let now = d.external_bytes();
    /* Clamp rather than saturate the report: a document Ruby cannot address
     * is not one we will see, and a truncated diff would unbalance `release`. */
    let (Ok(now_i), Ok(then_i)) = (isize::try_from(now), isize::try_from(d.reported)) else {
        return;
    };
    let diff = now_i.wrapping_sub(then_i);
    if diff != 0 {
        d.reported = now;
        // SAFETY: a Document method's frame, with the GVL held.
        unsafe { rb_sys::rb_gc_adjust_memory_usage(diff as rb_sys::ssize_t) };
    }
}

/// The base type: the kind-agnostic accessors (`doc_parsed`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: TypedType<DocData> = TypedType::base(c"Makiri::Document".as_ptr());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
pub static HTML_DOC_TYPE: TypedType<DocData> =
    TypedType::derived(c"Makiri::HTML::Document".as_ptr(), &DOC_TYPE);
pub static XML_DOC_TYPE: TypedType<DocData> =
    TypedType::derived(c"Makiri::XML::Document".as_ptr(), &DOC_TYPE);

/// Which leaf class a Document wrapper is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    Html,
    Xml,
}

/// A Document wrapper allocated before the content it will own, and the only
/// way a Document gets that content.
///
/// The order is the point. Allocating the Ruby wrapper can raise
/// (`NoMemoryError`), and a raise `longjmp`s past Rust destructors, so a parse
/// result held at that moment would leak. So the wrapper is made FIRST - while
/// nothing needs freeing - and the parsed handle goes in afterwards through
/// [`install`](Self::install), which also reports the arena to the GC. That
/// report is not optional (see `account_document`), and the parse entries used
/// to make it by hand, each one a place to forget it.
pub struct DocumentShell(VALUE);

impl DocumentShell {
    pub fn new(kind: DocKind) -> DocumentShell {
        let (klass, ty) = match kind {
            DocKind::Html => (crate::init::CLASS_HTML_DOCUMENT.raw(), &HTML_DOC_TYPE),
            DocKind::Xml => (crate::init::CLASS_XML_DOCUMENT.raw(), &XML_DOC_TYPE),
        };
        /* The errors array is built before the wrap and kept in a local, so the
         * conservative stack scan pins it across the wrap's allocation and the
         * store closure does not allocate (an allocation there could raise
         * NoMemoryError while the caller holds a live resource). */
        let errors = crate::bridge::ruby::array_new();
        // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
        DocumentShell(unsafe {
            ty.wrap(
                klass,
                |d| {
                    d.parsed = core::ptr::null_mut();
                    d.reported = 0;
                },
                |d| d.errors = errors.as_raw(),
            )
        })
    }

    /// Give the Document its parsed handle - the GC owns it from here - and
    /// report the arena's size.
    pub fn install(self, parsed: Box<Parsed>) -> Value {
        // SAFETY: `self.0` is a Document wrapper (the base type matches either
        // leaf) that has no handle yet, so nothing is overwritten.
        unsafe {
            let d = DOC_TYPE.known_ptr(value(self.0));
            (*d).parsed = Box::into_raw(parsed);
        }
        account_document(self.0);
        // SAFETY: a live Document.
        unsafe { value(self.0) }
    }
}

/* ------------------------------------------------------------------ *
 * lexbor <-> wrapper accessors                                       *
 * ------------------------------------------------------------------ */

/// The XML arena behind a parsed handle, or null for an HTML one.
///
/// # Safety
/// `p` must be a live handle.
pub(in crate::bridge) unsafe fn parsed_xml_doc(p: *mut Parsed) -> *mut XmlDoc {
    // SAFETY: the caller's contract.
    unsafe { (*p).xml_doc() }
}

/// The Lexbor document behind an HTML Document. `Err(TypeError)` otherwise.
pub fn html_doc_unwrap(rb_doc: Value) -> Result<RawDoc, Error> {
    let d: &DocData = HTML_DOC_TYPE.get(&rb_doc)?;
    Ok(html_doc_of(d))
}

/// The Lexbor document of `rb_doc`, a VALUE already known to be an HTML
/// Document (a Document method's receiver), borrowed for as long as the caller
/// borrows it.
pub fn html_doc(rb_doc: &Value) -> HtmlDoc<'_> {
    // SAFETY: a live HTML Document, kept alive by `rb_doc`, which the caller
    // holds for the borrow.
    unsafe { html_doc_known(*rb_doc).as_doc() }
}

/// [`html_doc_unwrap`] for a VALUE already known to be an HTML Document.
pub fn html_doc_known(rb_doc: Value) -> RawDoc {
    html_doc_of(HTML_DOC_TYPE.get_known(&rb_doc))
}

fn html_doc_of(d: &DocData) -> RawDoc {
    /* An lxb_html_document_t leads with its lxb_dom_document_t, so this is a
     * downcast to the embedded base, not a reinterpretation. */
    // SAFETY: `d` is the data of a live HTML Document, whose handle it owns.
    unsafe { RawDoc::from_ptr((*d.parsed).html_doc().cast()).expect("live document") }
}

/// The parsed handle behind any Document. `Err(TypeError)` for a non-Document.
pub(in crate::bridge) fn doc_parsed(rb_doc: Value) -> Result<*mut Parsed, Error> {
    let d: &DocData = DOC_TYPE.get(&rb_doc)?;
    Ok(d.parsed)
}

/// [`doc_parsed`] for a VALUE already known to be a Document - a node's
/// keepalive Document, or the receiver of a Document method.
pub(in crate::bridge) fn doc_parsed_known(rb_doc: Value) -> *mut Parsed {
    DOC_TYPE.get_known(&rb_doc).parsed
}

/// Run `f` over the parsed handle behind a Document.
///
/// The `&mut Parsed` does not escape `f`, so the raw pointer stays in this
/// layer and no alias can outlive the call. `f` must not run Ruby that could
/// re-enter this document (the readers' closures copy, they do not call back).
pub(in crate::bridge) fn with_parsed<R>(
    rb_doc: Value,
    f: impl FnOnce(&mut Parsed) -> R,
) -> Result<R, Error> {
    let p = doc_parsed(rb_doc)?;
    // SAFETY: under the GVL, and the borrow is confined to `f`.
    Ok(unsafe { f(&mut *p) })
}

/// [`with_parsed`] for a VALUE already known to be a Document.
pub(in crate::bridge) fn with_parsed_known<R>(
    rb_doc: Value,
    f: impl FnOnce(&mut Parsed) -> R,
) -> R {
    let p = doc_parsed_known(rb_doc);
    // SAFETY: as `with_parsed`.
    unsafe { f(&mut *p) }
}

/// The kind-AGNOSTIC raw node pointer (the base type, so HTML or XML), as an
/// opaque `*mut c_void`. Only for the few sites where the representation is
/// irrelevant (identity comparison) or already guaranteed by an external
/// same-document check (the XPath context node).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to Lexbor's.
pub fn node_raw(rb_node: Value) -> Result<*mut c_void, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        let parsed = doc_parsed(rb_node)?;
        // SAFETY: a Document's handle lives as long as the Document, and an XML
        // arena's document node is read, not written.
        unsafe {
            if (*parsed).is_xml() {
                let xdoc = parsed_xml_doc(parsed);
                return Ok(if xdoc.is_null() {
                    core::ptr::null_mut()
                } else {
                    (*xdoc).doc_node().to_token() as *mut c_void
                });
            }
        }
        return Ok(html_doc_unwrap(rb_node)?.as_ptr());
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    let nd: &NodeData = NODE_DATA_TYPE.get(&rb_node)?;
    Ok(nd.node)
}

/// Which representation `v` wraps ([`NodeRepr`]).
pub fn node_repr(v: Value) -> NodeRepr {
    if HTML_NODE_TYPE.is(v) {
        NodeRepr::Html
    } else if XML_NODE_TYPE.is(v) {
        NodeRepr::Xml
    } else {
        NodeRepr::Other
    }
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub fn node_identity(rb_node: Value) -> Result<usize, Error> {
    Ok(node_raw(rb_node)? as usize)
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub fn keepalive_document(rb_node: Value) -> Result<Value, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        return Ok(rb_node);
    }
    let nd: &NodeData = NODE_DATA_TYPE.get(&rb_node)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
}

/* ---- the document's mutation gate ---- */

/// `Err(Makiri::Error)` while an evaluation with a handler is reading `rb_doc`.
/// Every mutator checks this before it changes anything.
pub fn ensure_document_mutable(rb_doc: Value) -> Result<(), Error> {
    with_parsed_known(rb_doc, |p| {
        if p.evaluating != 0 {
            return Err(makiri_error("cannot modify a document while evaluating XPath over it (re-entrant mutation from a handler)",
            ));
        }
        /* The source offsets are stamped lazily, and this is the LAST moment
         * the tree is still the one the parser built. A walk after the edit
         * would pair elements with the wrong tokens, and `#line` must never
         * answer a wrong line - so it happens here, before the mutation. Only
         * the first mutation pays; there is nothing pending afterwards. */
        // SAFETY: the document this handle owns, not yet modified - that is
        // what this gate is called to decide.
        unsafe { p.assign_positions() };
        Ok(())
    })
}

/// Drop the DOM and text indexes so the next query rebuilds them.
pub fn invalidate_indexes(rb_doc: Value) {
    with_parsed_known(rb_doc, |p| p.invalidate_indexes());
}

/* ------------------------------------------------------------------ *
 * the evaluation guard                                                *
 * ------------------------------------------------------------------ */

/// Marks a document as read by an XPath evaluation that can run Ruby - one with
/// a handler - for as long as it lives. Nested evaluations stack.
///
/// The engine borrows names, attribute values and index slices out of the
/// document for the whole walk, and a handler runs arbitrary Ruby in the middle
/// of it. Lexbor frees an attribute's old value when a new one is set
/// (`lxb_dom_attr_set_value`), and a mutation drops the indexes, so a handler
/// that edited the same document could leave the evaluator reading freed
/// memory. Every mutator checks [`ensure_document_mutable`]
/// first, so that borrow is never invalidated under a suspended walk.
pub struct DocumentEvaluation(
    /// The Document the count belongs to. Holding it is what keeps the parsed
    /// handle valid: a guard lives on the machine stack, which Ruby's collector
    /// scans, so the Document cannot be collected while one is alive.
    Value,
);

impl DocumentEvaluation {
    pub fn enter(rb_doc: Value) -> Result<Self, Error> {
        with_parsed(rb_doc, |p| p.evaluating += 1)?;
        Ok(DocumentEvaluation(rb_doc))
    }
}

impl Drop for DocumentEvaluation {
    fn drop(&mut self) {
        with_parsed_known(self.0, |p| p.evaluating -= 1);
        /* Read the Document here, so the guard demonstrably holds it: the field
         * is there to keep it reachable, and a field nothing reads is one the
         * compiler is free to treat as absent. */
        core::hint::black_box(self.0);
    }
}
