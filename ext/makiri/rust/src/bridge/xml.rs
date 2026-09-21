//! The Ruby <-> XML-arena DOM seam (the XML counterpart of [`crate::bridge::html`]).
//!
//! A Ruby XML node is an arena [`NodeId`] behind a TypedData wrapper; turning a
//! `Value` into that id, and running the Ruby-free mutation primitives over the
//! arena, is the same kind of unsafe boundary the HTML side has - so it lives
//! here, under `bridge`, and the glue layer stays free of it.
//!
//! The rules themselves (name well-formedness, the XML character class,
//! namespace resolution, what may be a child of what) live in the Ruby-free
//! `crate::xml`; this layer coerces and verifies arguments, and maps the
//! resulting status to a Ruby exception.
//!
//! **Detach, never destroy.** A removed node is unlinked, not freed, so a live
//! Ruby wrapper for it stays valid; the arena owns the memory and outlives every
//! wrapper through the Document.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{prelude::*, Error, Ruby, Value};

use crate::bridge::html::html_node_unwrap;
use crate::bridge::ruby::{check_frozen, nil, value};
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::*;
use crate::bridge::wrapper::{
    ensure_document_mutable, node_repr, DocKind, DocumentShell, NodeRepr,
};
use crate::bridge::xml_decode::xml_decode_input_value;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT, EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::init::{
    CLASS_XML_ATTR, CLASS_XML_CDATA_SECTION, CLASS_XML_COMMENT, CLASS_XML_DOCUMENT_FRAGMENT,
    CLASS_XML_DOCUMENT_TYPE, CLASS_XML_ELEMENT, CLASS_XML_NODE, CLASS_XML_PROCESSING_INSTRUCTION,
    CLASS_XML_TEXT,
};
use crate::lexbor::adapter::cross_import::cross_html_to_xml;
use crate::xml::model::{
    Document as XmlDoc, Limits as XmlLimits, MutStatus, NodeId, NodeType, Status,
};
use crate::xml::mutate::{clone_node, copy_node_from, import_subtree, remove as remove_node};
use crate::xml::tree;

fn is_a(v: Value, klass: &crate::init::RbConst) -> bool {
    v.is_kind_of(klass.class())
}

/* ------------------------------------------------------------------ *
 * the XML node front door                                            *
 * ------------------------------------------------------------------ */

/// Wrap an arena node token into its `Makiri::XML::*` leaf.
///
/// An invalid token becomes nil, and the DOCUMENT node maps back onto the Ruby
/// Document rather than getting a second wrapper, so the arena has exactly one
/// owner. The token resolves through `document`'s arena, where a stale or
/// foreign id reads as no node.
pub fn wrap_xml_node(node: *mut core::ffi::c_void, document: Value) -> Value {
    let id = NodeId::from_token(node as usize);
    if id.is_invalid() {
        return nil();
    }
    let ty = arena_ref(&document).type_(id);
    if ty == Some(NodeType::Document) {
        return document;
    }
    let klass = match ty {
        Some(NodeType::Element) => CLASS_XML_ELEMENT.raw(),
        Some(NodeType::Attribute) => CLASS_XML_ATTR.raw(),
        Some(NodeType::Text) => CLASS_XML_TEXT.raw(),
        Some(NodeType::CData) => CLASS_XML_CDATA_SECTION.raw(),
        Some(NodeType::Comment) => CLASS_XML_COMMENT.raw(),
        Some(NodeType::Pi) => CLASS_XML_PROCESSING_INSTRUCTION.raw(),
        Some(NodeType::Doctype) => CLASS_XML_DOCUMENT_TYPE.raw(),
        Some(NodeType::Fragment) => CLASS_XML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_XML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `TypedType::wrap`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    unsafe {
        value(XML_NODE_TYPE.wrap(
            klass,
            |nd| nd.node = node,
            |nd| nd.document = document.as_raw(),
        ))
    }
}

/// The arena node token behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(rb_self: Value) -> Result<*mut core::ffi::c_void, Error> {
    if rb_self.is_kind_of(CLASS_XML_DOCUMENT.class()) {
        XML_DOC_TYPE.get(&rb_self)?; /* TypeError for any other Document */
        // SAFETY: the arena a live XML Document owns.
        let node = unsafe { (*doc_of(rb_self)).doc_node() };
        return Ok(node.to_token() as *mut core::ffi::c_void);
    }
    let nd: &NodeData = XML_NODE_TYPE.get(&rb_self)?;
    Ok(nd.node)
}

/// The XML arena behind a value checked to be an XML Document:
/// `Err(TypeError)` for anything else. For a receiver not yet established as
/// one; [`doc_of`] is for a document already known to be.
pub fn xml_doc_unwrap(rb_doc: Value) -> Result<*mut XmlDoc, Error> {
    XML_DOC_TYPE.get(&rb_doc)?;
    Ok(doc_of(rb_doc))
}

/// The XML arena behind an XML Document.
///
/// Every XML Document HAS one: `DocumentShell::install` gives it the arena
/// before the Document reaches Ruby, and nothing takes it away. So there is no
/// "no arena" case to handle, and a null here is a broken invariant - it
/// panics (unwinding to `fatal` / `Makiri::InternalError`) rather than being
/// read through.
pub fn doc_of(document: Value) -> *mut XmlDoc {
    let arena = xml_arena_known(document);
    assert!(!arena.is_null(), "an XML Document without its arena");
    arena
}

/// The arena behind `document`, borrowed for as long as the caller borrows the
/// VALUE - which it holds, and which keeps the Document alive.
fn arena_ref(document: &Value) -> &XmlDoc {
    // SAFETY: the Document's `DocData` owns the arena, alive while `document`
    // is held; only read here.
    unsafe { &*doc_of(*document) }
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node
/// at the type boundary, like [`xml_node_unwrap`].
pub fn xml_node_document(rb_self: Value) -> Result<Value, Error> {
    if rb_self.is_kind_of(CLASS_XML_DOCUMENT.class()) {
        return Ok(rb_self);
    }
    let nd: &NodeData = XML_NODE_TYPE.get(&rb_self)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
}

/* ------------------------------------------------------------------ *
 * the XML receiver and the node front door                           *
 * ------------------------------------------------------------------ */

/// A method receiver already checked to be an XML node or XML Document; see the
/// HTML twin, `HtmlSelf`.
#[derive(Clone, Copy)]
pub struct XmlSelf {
    pub value: Value,
    pub id: NodeId,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl magnus::TryConvert for XmlSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let id = unwrap(value)?;
        let document = xml_node_document(value)?;
        Ok(XmlSelf {
            value,
            id,
            document,
        })
    }
}

impl XmlSelf {
    /// The arena behind the receiver's Document, borrowed (readers) for as long
    /// as the receiver is.
    pub fn doc_ref(&self) -> &XmlDoc {
        arena_ref(&self.document)
    }
}

/// [`xml_node_unwrap`] with the node id typed.
pub fn unwrap(v: Value) -> Result<NodeId, Error> {
    Ok(NodeId::from_token(xml_node_unwrap(v)? as usize))
}

/// The XML arena behind `document`, for a WRITE: refused while an XPath
/// evaluation with a handler is reading the document.
///
/// The evaluator holds the arena as `&Document` for the whole walk, and a
/// write - a new node, a new byte span - can grow the arena's vectors under the
/// slices it borrowed. Every path that hands out a `&mut` goes through here, so
/// the one mutation gate covers the factories and the imports as well as the
/// tree edits.
fn arena_mut(document: Value) -> Result<*mut XmlDoc, Error> {
    ensure_document_mutable(document)?;
    Ok(doc_of(document))
}

/// Wrap an arena node under `document`, its XML Document.
pub fn wrap(node: NodeId, document: Value) -> Value {
    wrap_xml_node(node.to_token() as *mut core::ffi::c_void, document)
}

/// Wrap a node reached from a checked receiver, under its Document.
pub fn xml_wrap_rel_value(this: XmlSelf, rel: NodeId) -> Value {
    wrap(rel, this.document)
}

/// The exception for a non-OK mutation status; [`MutStatus::Ok`] is `Ok`.
/// A translation's `Result` as the Ruby error [`xml_mut_check`] maps its status
/// to. An `Err(MutStatus::Ok)` cannot be built by the translators, but is
/// refused rather than read as success.
pub fn xml_mut_result<T>(r: Result<T, MutStatus>) -> Result<T, Error> {
    r.or_else(|st| {
        xml_mut_check(st)?;
        Err(makiri_error("XML translation failed without a status"))
    })
}

pub fn xml_mut_check(st: MutStatus) -> Result<(), Error> {
    let msg: &str = match st {
        MutStatus::Ok => return Ok(()),
        MutStatus::Oom => "out of memory mutating XML",
        MutStatus::BadName => {
            let ruby = Ruby::get().expect("under the GVL");
            return Err(Error::new(
                ruby.exception_arg_error(),
                "not a well-formed XML name",
            ));
        }
        MutStatus::BadChars => "value contains a character or sequence not permitted in XML",
        MutStatus::UnboundNs => "namespace prefix is not bound in this scope",
        MutStatus::Type => "operation unsupported for this node type",
        MutStatus::Cycle => "cannot insert a node into its own subtree",
        MutStatus::Hierarchy => {
            "invalid placement (an attribute/document node cannot be a tree child, a document \
allows a single root element, and a sibling target must have a parent)"
        }
        MutStatus::BadNsDecl => "cannot bind a namespace prefix to the empty namespace",
        MutStatus::Internal => "internal error mutating XML (no document)",
        /* The document's own budget, not the machine's memory - so the same
         * exception a parse raises for the same cause. */
        MutStatus::Limit => {
            return Err(Error::new(
                EXC_XML_LIMIT_EXCEEDED.exception(),
                "XML document exceeded its byte or node budget",
            ))
        }
    };
    Err(makiri_error(msg))
}

/* ------------------------------------------------------------------ */
/* lending the arena for an edit                                      */
/* ------------------------------------------------------------------ */

/// Run `f` on `document`'s arena to build a DETACHED node - the factories'
/// gate, and the one way a caller outside this module gets `&mut` to an XML
/// arena without a receiver.
///
/// A detached node is not in the tree, so it is not in the name index and there
/// is no receiver to check for frozenness. A TREE EDIT is [`Editing`]: it needs
/// both, and the only way to get the `&mut` for one is to spend the token
/// `begin_edit` hands out. The name says which of the two this is, because a
/// mutator reaching for "the arena, mutably" would otherwise skip the checks
/// simply by asking for the wrong thing - which is how the XML side came to have
/// nine sites that correctly skip the index invalidation and eight that must
/// not, with nothing but a reader's judgement telling them apart.
///
/// Refused while an XPath evaluation with a handler reads the document (see
/// [`arena_mut`]). The `&mut` lives for `f` alone, and `f` must not run Ruby:
/// the arena is `Vec`s, so Ruby code that read or wrote this same document
/// meanwhile would alias the borrow. That is why a method converts and checks
/// its arguments FIRST and only then calls this, with nothing but engine calls
/// inside - which are Ruby-free by construction.
pub fn with_arena_for_new_node<R>(
    document: Value,
    f: impl FnOnce(&mut XmlDoc) -> R,
) -> Result<R, Error> {
    let xd = arena_mut(document)?;
    // SAFETY: a live arena of `document`, which the caller holds; cleared for
    // writing above, and borrowed only for `f`, which runs no Ruby.
    Ok(f(unsafe { &mut *xd }))
}

/// The receiver cleared for an edit, and the PROOF of it.
///
/// [`begin_edit`] is the only way to build one and [`Editing::with_arena`] the
/// only way to spend it, so a tree edit cannot reach the arena without the two
/// things that must happen first: the frozen check, and dropping the name index
/// the edit is about to invalidate.
///
/// [`with_arena_for_new_node`] stays for the FACTORIES, which build a detached node -
/// not in the tree, so not in the index, and with no receiver to freeze. The
/// HTML side has had this as one `edit` gate all along; XML had the `&mut` gate
/// and the invalidate gate as two separate calls, and whether a site needed the
/// second was a judgement the reader had to make at each of seventeen.
pub struct Editing {
    document: Value,
    id: NodeId,
}

impl Editing {
    /// The node being edited.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Its document, for the Ruby-side work an edit does around the arena call.
    pub fn document(&self) -> Value {
        self.document
    }

    /// Lend the arena for the change. `f` runs no Ruby (see [`with_arena_for_new_node`]),
    /// so every argument is converted BEFORE this - which is also why the frozen
    /// check is in `begin_edit` rather than here: it must stay ahead of the
    /// argument conversion, so a frozen receiver is reported before a bad
    /// argument, as it always was.
    pub fn with_arena<R>(&self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        let id = self.id;
        with_arena_for_new_node(self.document, |d| f(d, id))
    }
}

/// The receiver cleared for an edit - not frozen, its document not under
/// evaluation - with the document's name index dropped, since the edit is
/// about to change what it indexes.
pub fn begin_edit(this: XmlSelf) -> Result<Editing, Error> {
    check_frozen(this.value)?;
    with_arena_for_new_node(this.document, XmlDoc::invalidate_name_index)?;
    Ok(Editing {
        document: this.document,
        id: this.id,
    })
}

/// A String argument verified as an engine string - valid UTF-8, no NUL - and
/// short enough for an arena span (4 GiB).
pub fn verified_text(v: Value, what: &core::ffi::CStr) -> Result<RubyText, Error> {
    let t = ruby_verified_text(v, what)?;
    if u32::try_from(t.len()).is_err() {
        return Err(makiri_error("string too long for an XML node (max 4 GiB)"));
    }
    Ok(t)
}

/// [`verified_text`] for an optional argument: nil is absent.
pub fn verified_text_opt(v: Value, what: &core::ffi::CStr) -> Result<RubyText, Error> {
    if v.is_nil() {
        return Ok(RubyText::absent());
    }
    verified_text(v, what)
}

/* ------------------------------------------------------------------ */
/* documents: parsing, readers, fragments                             *
 * ------------------------------------------------------------------ */

/// A `Makiri::XML::SyntaxError`-family error for a parse status.
fn parse_status_error(status: Status, unit: Unit) -> Error {
    match status {
        Status::Syntax => Error::new(EXC_XML_SYNTAX_ERROR.exception(), unit.malformed()),
        Status::Limit => Error::new(EXC_XML_LIMIT_EXCEEDED.exception(), unit.budget()),
        Status::Unsupported => Error::new(
            EXC_XML_SYNTAX_ERROR.exception(),
            "unsupported DTD construct: Makiri does not apply attribute defaults or \
             non-CDATA attribute types, expand parameter entities, or expand entities \
             a DTD declares",
        ),
        /* `Ok` never reaches here (it means no failure); the rest are the
         * generic "failed to parse" bucket. */
        Status::Ok | Status::Oom | Status::Internal => makiri_error(unit.failed()),
    }
}

/// Which entry point failed. The two carry their own wording rather than one
/// composed string: the document path says "malformed XML" where the fragment
/// path says "malformed XML fragment", and the messages are observable.
#[derive(Clone, Copy)]
enum Unit {
    Document,
    Fragment,
}

impl Unit {
    fn malformed(self) -> &'static str {
        match self {
            Unit::Document => "malformed XML",
            Unit::Fragment => "malformed XML fragment",
        }
    }
    fn budget(self) -> &'static str {
        match self {
            Unit::Document => "XML document budget exceeded",
            Unit::Fragment => "XML fragment budget exceeded",
        }
    }
    fn failed(self) -> &'static str {
        match self {
            Unit::Document => "failed to parse XML document",
            Unit::Fragment => "failed to parse XML fragment",
        }
    }
}

/// Parse XML source, releasing the GVL, and wrap the result as a Document.
///
/// Runs the strict decode under the GVL first (invalid UTF-8, an undecodable
/// byte or a NUL all raise), then copies into a private buffer BEFORE the
/// wrapper exists, so no GC point can run between obtaining the decoded String
/// and copying it.
pub fn parse_xml_document(source: Value, limits: XmlLimits, budget: usize) -> Result<Value, Error> {
    let source = crate::bridge::ruby::string_of(source)?;
    let decoded = xml_decode_input_value(source.as_value(), budget)?;
    let src = crate::bridge::string::ruby_string_bytes(decoded)?;

    /* The wrapper first, while nothing needs freeing (see DocumentShell). The
     * source is already copied, so this Ruby allocation cannot disturb it. */
    let shell = DocumentShell::new(DocKind::Xml);

    /* Ruby-free from here: only the copied bytes and the limits cross. */
    let (result, status) =
        crate::bridge::gvl::without_gvl(|| match tree::parse_ex(src.as_slice(), Some(&limits)) {
            Ok(doc) => (Box::into_raw(doc), Status::Ok),
            Err(status) => (core::ptr::null_mut(), status),
        });
    drop(src);

    if result.is_null() {
        return Err(parse_status_error(status, Unit::Document));
    }
    // SAFETY: `result` is the arena the parse just returned, owned by no one.
    let arena = unsafe { Box::from_raw(result) };
    /* `src` is gone, so the collection `install`'s GC report may trigger
     * disturbs nothing. */
    Ok(shell.install_xml(arena))
}

/// `Document#root` for an XML document: the root element, or nil.
pub fn document_root(ruby: &Ruby, rb_self: Value) -> Value {
    match arena_ref(&rb_self).root {
        Some(n) => wrap(n, rb_self),
        None => ruby.qnil().as_value(),
    }
}

/// `Document#internal_subset` for an XML document: the DOCTYPE node, or nil.
pub fn document_internal_subset(ruby: &Ruby, rb_self: Value) -> Value {
    match arena_ref(&rb_self).doctype {
        Some(n) => wrap(n, rb_self),
        None => ruby.qnil().as_value(),
    }
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
pub fn new_empty_xml_document() -> Result<Value, Error> {
    let shell = DocumentShell::new(DocKind::Xml);
    let arena = XmlDoc::create(None, 0)
        .map_err(|_| makiri_error("out of memory allocating XML document"))?;
    Ok(shell.install_xml(arena))
}

/// Strict-decode `source` and parse it as a fragment into `document`'s arena,
/// returning the fragment node.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
pub fn fragment_into(
    document: Value,
    source: Value,
    inherit_doc_ns: bool,
) -> Result<NodeId, Error> {
    let xdoc = arena_mut(document)?;
    let source = crate::bridge::ruby::string_of(source)?;
    // SAFETY: a live arena; the decode only reads its `max_bytes`.
    let decoded = xml_decode_input_value(source.as_value(), unsafe { (*xdoc).max_bytes })?;
    let src = crate::bridge::string::ruby_string_bytes(decoded)?;
    // SAFETY: the arena is live and mutable for this call, under the GVL.
    tree::parse_fragment(unsafe { &mut *xdoc }, src.as_slice(), inherit_doc_ns)
        .map_err(|status| parse_status_error(status, Unit::Fragment))
}

/* ------------------------------------------------------------------ */
/* attribute lookup                                                   *
 * ------------------------------------------------------------------ */

/// The attribute of `el` whose qualified name is exactly the verified `name`.
///
/// `None` for a non-element (the name is then not even verified, matching the
/// readers' nil-returning behaviour). The name is converted BEFORE the arena is
/// borrowed, because its `to_str` is Ruby code and it may edit this same
/// document.
pub fn find_attribute(this: XmlSelf, name: Value) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.doc_ref().type_(id) != Some(NodeType::Element) {
        return Ok(None);
    }
    let nv = ruby_verified_text(name, c"attribute name")?;
    // SAFETY: the bytes are the verified view's, live across the lookup, and
    // nothing below runs Ruby.
    let bytes = unsafe { nv.bytes() };
    Ok(find_attribute_bytes(this.doc_ref(), id, bytes))
}

/// The attribute of `el` whose qualified name is `name`.
///
/// Namespace declarations included: in the DOM an `xmlns` / `xmlns:p` is an
/// attribute, so `node["xmlns:p"]` reads it as `getAttribute` does. XPath's data
/// model is the one that hides them (`xml::xpath` skips them on the attribute
/// axis), which is why `@xmlns:p` finds nothing while this does.
fn find_attribute_bytes(d: &XmlDoc, el: NodeId, name: &[u8]) -> Option<NodeId> {
    if d.type_(el) != Some(NodeType::Element) {
        return None;
    }
    let mut a = d.attrs(el);
    while let Some(id) = a {
        if d.qname(id) == name {
            return Some(id);
        }
        a = d.next(id);
    }
    None
}

/* ------------------------------------------------------------------ */
/* two arenas at once: adopting and importing                         */
/* ------------------------------------------------------------------ */
/* The node methods live in `glue::xml_node::mutate`. What stays here is what
 * holds two arenas - or an arena and a Lexbor document - at the same time,
 * which `with_arena_for_new_node`'s one-at-a-time lending cannot express. */

/// A node copied in from another document, still to be taken out of it: the
/// second half of the move `appendChild` performs across arenas. It carries
/// the source arena and node [`incoming_node`] already resolved, so finishing
/// cannot fail - there is nothing left to look up.
pub struct Adoption {
    src_doc: *mut XmlDoc,
    src: NodeId,
    /// The source node's wrapper, which keeps its document - and so
    /// `src_doc` - alive until the adoption is finished.
    _keep: Value,
}

impl Adoption {
    /// Empty the node out of its old document, whose name index goes with it.
    pub fn finish(self) {
        // SAFETY: `src_doc` is the live arena `incoming_node` found and cleared
        // for writing; `_keep` holds it, and the caller ran only engine code
        // on the OTHER arena since.
        let sdoc = unsafe { &mut *self.src_doc };
        if sdoc.type_(self.src) == Some(NodeType::Fragment) {
            while let Some(c) = sdoc.first_child(self.src) {
                remove_node(sdoc, c);
            }
        } else {
            remove_node(sdoc, self.src);
        }
        sdoc.invalidate_name_index();
    }
}

/// `arg` as a node of `target_doc`'s arena: itself when it already lives there
/// (a move), or a copy imported from its own document plus the [`Adoption`]
/// that takes it out of there once it is placed.
pub fn incoming_node(target_doc: Value, arg: Value) -> Result<(NodeId, Option<Adoption>), Error> {
    if !is_a(arg, &CLASS_NODE) || !is_a(xml_node_document(arg)?, &CLASS_XML_DOCUMENT) {
        return Err(Error::new(
            Ruby::get_with(arg).exception_type_error(),
            "expected a Makiri::XML node (NodeSet / String arguments are a later phase)",
        ));
    }
    /* Inserting `arg` relinks IT - its parent, prev and next all change, and an
     * adoption takes it out of its own document - so a frozen argument is a
     * frozen node being modified, which the receiver check alone let through:
     * `a.remove` raised and `b.add_child(a)` did not, for the same effect on `a`.
     *
     * This reaches the nodes the caller NAMED. A fragment argument splices its
     * children, and those cannot be checked: frozenness is a property of a Ruby
     * object and the arena has no map from a node back to its wrapper (see
     * CLAUDE.md on why a per-node wrapper cache was rejected). A child the caller
     * never named is out of reach by construction, not by choice. */
    check_frozen(arg)?;
    let src = unwrap(arg)?;
    let src_document = xml_node_document(arg)?;
    if src_document.as_raw() == target_doc.as_raw() {
        return Ok((src, None)); /* same arena -> move */
    }
    let xd = arena_mut(target_doc)?;
    /* The source changes too - adopting takes the node out of it. */
    let src_doc = arena_mut(src_document)?;
    // SAFETY: two distinct live arenas (the documents differ), both cleared
    // for writing; the source is only read here.
    let copy = xml_mut_result(unsafe { import_subtree(&mut *xd, &*src_doc, src) })?;
    Ok((
        copy,
        Some(Adoption {
            src_doc,
            src,
            _keep: arg,
        }),
    ))
}

/// `Document#import_node`'s copy: `node_v` - XML from any document, or HTML -
/// copied, detached, into the XML Document `rb_self`. The source is untouched.
pub fn import_copy(rb_self: Value, node_v: Value, deep: bool) -> Result<NodeId, Error> {
    xml_doc_unwrap(rb_self)?; /* TypeError for anything but an XML Document */
    let xd = arena_mut(rb_self)?;
    let copy = match node_repr(node_v) {
        NodeRepr::Xml => {
            /* Read, not written: the copy goes into the receiver's arena. */
            let src_doc = doc_of(xml_node_document(node_v)?);
            let src = unwrap(node_v)?;
            if src_doc == xd {
                /* Same arena: the single-`&mut` clone path. */
                // SAFETY: the target arena, which is the source here.
                xml_mut_result(unsafe { clone_node(&mut *xd, src, deep) })?
            } else {
                // SAFETY: two distinct live arenas.
                xml_mut_result(unsafe { copy_node_from(&mut *xd, &*src_doc, src, deep) })?
            }
        }
        NodeRepr::Html => {
            let src = html_node_unwrap(node_v)?;
            // SAFETY: the target arena, and the HTML source node - live, and
            // nothing restructures its document during the copy.
            xml_mut_result(unsafe { cross_html_to_xml(&mut *xd, src, deep) })?
        }
        NodeRepr::Other => {
            return Err(Error::new(
                Ruby::get_with(node_v).exception_type_error(),
                "import_node expects a Makiri node",
            ))
        }
    };
    Ok(copy)
}
