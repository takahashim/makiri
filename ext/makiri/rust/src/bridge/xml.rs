//! The Ruby <-> XML-arena DOM seam (the XML counterpart of [`crate::bridge::lexbor`]).
//!
//! A Ruby XML node is an arena [`NodeId`] behind a TypedData wrapper; turning a
//! `Value` into that id, and running the Ruby-free mutation primitives over the
//! arena, is the same kind of unsafe boundary the HTML side has - so it lives
//! here, under `bridge`, and the glue layer stays free of it.
//!
//! The rules themselves (name well-formedness, the XML character class,
//! namespace resolution, what may be a child of what) live in the Ruby-free
//! `xml/mkr_xml_*`; this layer coerces and verifies arguments, and maps the
//! resulting status to a Ruby exception.
//!
//! **Detach, never destroy.** A removed node is unlinked, not freed, so a live
//! Ruby wrapper for it stays valid; the arena owns the memory and outlives every
//! wrapper through the Document.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};

use crate::bridge::lexbor::{
    doc_of, ensure_document_mutable, html_node_unwrap, node_kind, wrap_document, wrap_xml_node,
    xml_doc_ref, xml_node_document, xml_node_unwrap,
};
use crate::bridge::ruby::{check_frozen, value};
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::xml_decode::xml_decode_input_value;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT, EXC_ERROR, EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::lexbor::adapter::cross_import::cross_html_to_xml;
use crate::lexbor::adapter::post_parse::Parsed;
use crate::xml::api::*;
use crate::xml::model::{Doc as XmlDoc, Limits as XmlLimits, MutStatus, NodeId, NodeType, Status};

/// `NodeKind`.
const KIND_HTML: core::ffi::c_int = 1;
const KIND_XML: core::ffi::c_int = 2;

fn error_class() -> magnus::ExceptionClass {
    EXC_ERROR.exception()
}

fn is_a(v: Value, klass: &crate::init::RbConst) -> bool {
    v.is_kind_of(klass.class())
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
    /// The arena behind the receiver's Document, as a mutable handle (mutators).
    pub fn doc(self) -> *mut XmlDoc {
        doc_of(self.document)
    }

    /// The arena behind the receiver's Document, borrowed (readers).
    pub fn doc_ref(&self) -> &XmlDoc {
        xml_doc_ref(self.document)
    }
}

/// [`xml_node_unwrap`] with the node id typed.
pub fn unwrap(v: Value) -> Result<NodeId, Error> {
    Ok(NodeId::from_token(xml_node_unwrap(v)? as usize))
}

/// The keepalive Document of an XML node. `Err(TypeError)` for an HTML node.
pub fn node_document(v: Value) -> Result<Value, Error> {
    xml_node_document(v)
}

/// The XML document behind a node wrapper. `Err(TypeError)` for an HTML node.
pub fn doc(v: Value) -> Result<*mut XmlDoc, Error> {
    Ok(doc_of(xml_node_document(v)?))
}

/// Wrap an arena node under `document`, its XML Document.
pub fn wrap(node: NodeId, document: Value) -> Value {
    wrap_xml_node(node.to_token() as *mut core::ffi::c_void, document)
}

/// Wrap a node reached from a checked receiver, under its Document.
pub fn xml_wrap_rel_value(this: XmlSelf, rel: NodeId) -> Value {
    wrap(rel, this.document)
}

pub use crate::xml::api::xml_clone_node;
pub use crate::xml::api::xml_copy_node;
pub use crate::xml::api::xml_import_subtree;

/// The exception for a non-OK mutation status; [`MutStatus::Ok`] is `Ok`.
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
    };
    Err(Error::new(error_class(), msg))
}

/* ------------------------------------------------------------------ */
/* helpers                                                            */
/* ------------------------------------------------------------------ */

/// The arena behind a node's document.
fn xdoc(v: Value) -> Result<*mut XmlDoc, Error> {
    let document = node_document(v)?;
    // SAFETY: the handle of `v`'s own Document, which `v` keeps alive.
    Ok(unsafe { crate::bridge::lexbor::parsed_xml_doc(crate::bridge::lexbor::doc_parsed_known(document)) }
        as *mut XmlDoc)
}

/// A byte length as the arena's `uint32`, or an error.
fn u32_len(ruby: &Ruby, len: usize) -> Result<u32, Error> {
    u32::try_from(len).map_err(|_| {
        let _ = ruby;
        Error::new(error_class(), "string too long for an XML node (max 4 GiB)")
    })
}

/// Unwrap for mutation.
fn unwrap_mutable(this: XmlSelf) -> Result<NodeId, Error> {
    check_frozen(this.value)?;
    ensure_document_mutable(this.document)?;
    // SAFETY: the receiver's own arena, which the receiver keeps alive, and
    // nothing else holds a borrow of it here.
    unsafe { xml_name_index_invalidate(&mut *this.doc()) };
    Ok(this.id)
}

/// Verify a String argument and hand back its bytes plus the length the arena
/// wants.
fn verified(ruby: &Ruby, v: Value, what: &core::ffi::CStr) -> Result<(RubyText, u32), Error> {
    let t = ruby_verified_text(v, what)?;
    let n = u32_len(ruby, t.len())?;
    Ok((t, n))
}

/// The same for an optional argument: nil is (absent, 0).
fn verified_opt(ruby: &Ruby, v: Value, what: &core::ffi::CStr) -> Result<(RubyText, u32), Error> {
    if v.is_nil() {
        return Ok((RubyText::absent(), 0));
    }
    verified(ruby, v, what)
}

/* ------------------------------------------------------------------ */
/* documents: parsing, readers, fragments                             *
 * ------------------------------------------------------------------ */

/// A `Makiri::XML::SyntaxError`-family error for a parse status.
fn parse_status_error(status: Status, unit: Unit) -> Error {
    match status {
        Status::Syntax => Error::new(EXC_XML_SYNTAX_ERROR.exception(), unit.malformed()),
        Status::Limit => Error::new(EXC_XML_LIMIT_EXCEEDED.exception(), unit.budget()),
        Status::Version => Error::new(
            EXC_XML_SYNTAX_ERROR.exception(),
            "unsupported XML version (only XML 1.0 is supported)",
        ),
        /* `Ok` never reaches here (it means no failure); the rest are the
         * generic "failed to parse" bucket. */
        Status::Ok | Status::Oom | Status::Internal => {
            Error::new(EXC_ERROR.exception(), unit.failed())
        }
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

    /* Wrap an empty handle first, so a failure mid-parse still frees cleanly
     * through the GC. The source is already copied, so this Ruby allocation
     * cannot disturb it. */
    let Some(parsed) = Parsed::new_xml() else {
        return Err(Error::new(
            error_class(),
            "out of memory allocating XML document",
        ));
    };
    let parsed = Box::into_raw(parsed);
    // SAFETY: a fresh handle; the wrap hands it to the GC.
    let obj = unsafe { wrap_document(parsed) };

    /* Ruby-free from here: only the copied bytes and the limits cross. */
    let (result, status) = crate::bridge::gvl::without_gvl(|| {
        match crate::xml::api::xml_parse_ex(src.as_slice(), Some(&limits)) {
            Ok(doc) => (Box::into_raw(doc), Status::Ok),
            Err(status) => (core::ptr::null_mut(), status),
        }
    });
    drop(src);

    if result.is_null() {
        return Err(parse_status_error(status, Unit::Document));
    }
    // SAFETY: `parsed` is the handle behind `obj`, and `result` is the arena the
    // parse produced for it.
    unsafe {
        (*parsed).set_xml_doc(Box::from_raw(result));
        Ok(value(obj))
    }
}

/// `Document#root` for an XML document: the root element, or nil.
pub fn document_root(ruby: &Ruby, rb_self: Value) -> Value {
    let xdoc = doc_of(rb_self);
    if xdoc.is_null() {
        return ruby.qnil().as_value();
    }
    // SAFETY: a live XML Document's arena, kept alive by `rb_self`.
    match unsafe { (*xdoc).root } {
        Some(n) => wrap(n, rb_self),
        None => ruby.qnil().as_value(),
    }
}

/// `Document#internal_subset` for an XML document: the DOCTYPE node, or nil.
pub fn document_internal_subset(ruby: &Ruby, rb_self: Value) -> Value {
    let xdoc = doc_of(rb_self);
    if xdoc.is_null() || unsafe { (*xdoc).doctype.is_none() } {
        return ruby.qnil().as_value();
    }
    // SAFETY: as `document_root`.
    match unsafe { (*xdoc).doctype } {
        Some(n) => wrap(n, rb_self),
        None => ruby.qnil().as_value(),
    }
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
pub fn new_empty_xml_document() -> Result<Value, Error> {
    let Some(parsed) = Parsed::new_xml() else {
        return Err(Error::new(
            error_class(),
            "out of memory allocating XML document",
        ));
    };
    let parsed = Box::into_raw(parsed);
    // SAFETY: a fresh handle; the wrap hands it to the GC.
    let doc_obj = unsafe { wrap_document(parsed) };
    let xdoc = crate::xml::api::xml_doc_new()
        .map_err(|_| Error::new(error_class(), "out of memory allocating XML document"))?;
    // SAFETY: `parsed` is the handle behind `doc_obj`, live for this call.
    unsafe { (*parsed).set_xml_doc(xdoc) };
    Ok(unsafe { value(doc_obj) })
}

/// Strict-decode `source` and parse it as a fragment into `document`'s arena,
/// returning the fragment node.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
pub fn fragment_into(document: Value, source: Value, inherit_doc_ns: bool) -> Result<NodeId, Error> {
    let xdoc = doc_of(document);
    if xdoc.is_null() {
        return Err(Error::new(error_class(), "the document has no arena"));
    }
    let source = crate::bridge::ruby::string_of(source)?;
    // SAFETY: a live arena; the decode only reads its `max_bytes`.
    let decoded = xml_decode_input_value(source.as_value(), unsafe { (*xdoc).max_bytes })?;
    let src = crate::bridge::string::ruby_string_bytes(decoded)?;
    // SAFETY: the arena is live and mutable for this call, under the GVL.
    crate::xml::api::xml_parse_fragment(unsafe { &mut *xdoc }, src.as_slice(), inherit_doc_ns)
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
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self.
pub fn remove(this: XmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    if is_a(rb_self, &CLASS_XML_DOCUMENT) {
        return Err(Error::new(error_class(), "cannot remove the document node"));
    }
    let n = unwrap_mutable(this)?;
    // SAFETY: the receiver's own arena, live for this call.
    unsafe { xml_remove(&mut *this.doc(), n) };
    Ok(rb_self)
}

/// The element behind `rb_self`, or an error naming what was attempted.
fn element_for(this: XmlSelf) -> Result<NodeId, Error> {
    let n = unwrap_mutable(this)?;
    // SAFETY: the receiver's arena, read for this statement only.
    if unsafe { (*this.doc()).type_(n) } != Some(NodeType::Element) {
        return Err(Error::new(
            error_class(),
            "cannot set an attribute on a non-element node",
        ));
    }
    Ok(n)
}

/// `element[name] = value` -> value.
pub fn aset(ruby: &Ruby, this: XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    let n = element_for(this)?;
    let (nv, _) = verified(ruby, name, c"attribute name")?;
    let (vv, _) = verified(ruby, val, c"attribute value")?;
    let mut out = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the views are the caller's for the call.
    let st = unsafe { xml_set_attribute(&mut *this.doc(), n, nv.bytes(), vv.bytes(), &mut out) };
    xml_mut_check(st)?;
    Ok(val)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    let n = element_for(this)?;
    let (qv, _) = verified(ruby, qname, c"attribute qualified name")?;
    let (vv, _) = verified(ruby, val, c"attribute value")?;
    let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
    let mut out = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the views are the caller's for the call.
    let st = unsafe {
        xml_set_attribute_ns(
            &mut *this.doc(),
            n,
            nv.bytes(),
            qv.bytes(),
            vv.bytes(),
            &mut out,
        )
    };
    xml_mut_check(st)?;
    Ok(val)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    let rb_self = this.value;
    let n = unwrap_mutable(this)?;
    // SAFETY: the receiver's arena, live for this call.
    if unsafe { (*this.doc()).type_(n) } != Some(NodeType::Element) {
        return Ok(rb_self);
    }
    let (lv, _) = verified(ruby, local, c"attribute local name")?;
    let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
    // SAFETY: as above.
    unsafe { xml_remove_attribute_ns(&mut *this.doc(), n, nv.bytes(), lv.bytes()) };
    Ok(rb_self)
}

/// `element.delete(name)` / `#remove_attribute` -> self.
pub fn delete(ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let n = unwrap_mutable(this)?;
    // SAFETY: the receiver's arena, live for this call.
    if unsafe { (*this.doc()).type_(n) } != Some(NodeType::Element) {
        return Ok(rb_self);
    }
    let (nv, _) = verified(ruby, name, c"attribute name")?;
    // SAFETY: as above.
    unsafe { xml_remove_attribute(&mut *this.doc(), n, nv.bytes()) };
    Ok(rb_self)
}

/// `node.content = text` -> text.
pub fn set_content(ruby: &Ruby, this: XmlSelf, text: Value) -> Result<Value, Error> {
    let n = unwrap_mutable(this)?;
    let (tv, _) = verified(ruby, text, c"node content")?;
    // SAFETY: the receiver's arena, and the view is the caller's for the call.
    let st = unsafe { xml_set_content(&mut *this.doc(), n, tv.bytes()) };
    xml_mut_check(st)?;
    Ok(text)
}

/// `node.name = new_name` -> new_name.
pub fn set_name(ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Value, Error> {
    let n = unwrap_mutable(this)?;
    let (nv, _) = verified(ruby, name, c"node name")?;
    // SAFETY: the receiver's arena, and the view is the caller's for the call.
    let st = unsafe { xml_rename(&mut *this.doc(), n, nv.bytes()) };
    xml_mut_check(st)?;
    Ok(name)
}

/* ------------------------------------------------------------------ */
/* building: insertion                                                */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Child,
    Before,
    After,
    Replace,
}

/// Coerce `arg` to a node living in (or imported into) `target`'s arena.
unsafe fn incoming_node(
    ruby: &Ruby,
    xd: *mut XmlDoc,
    target_doc: Value,
    arg: Value,
) -> Result<(NodeId, Value), Error> {
    if !is_a(arg, &CLASS_NODE) || !is_a(node_document(arg)?, &CLASS_XML_DOCUMENT) {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::XML node (NodeSet / String arguments are a later phase)",
        ));
    }
    let src = unwrap(arg)?;
    if node_document(arg)?.as_raw() == target_doc.as_raw() {
        return Ok((src, ruby.qnil().as_value())); /* same arena -> move */
    }
    ensure_document_mutable(node_document(arg)?)?;
    let mut copy: NodeId = NodeId::INVALID;
    let src_doc = xdoc(arg)?;
    // SAFETY: `xd` is the target arena and `src_doc` the source; they differ.
    xml_mut_check(unsafe { xml_import_subtree(&mut *xd, &*src_doc, src, &mut copy) })?;
    Ok((copy, arg))
}

/// Finish the adoption by emptying the node out of its old document.
fn adopt_finish(arg: Value) {
    if arg.is_nil() {
        return;
    }
    let Ok(src) = unwrap(arg) else {
        return;
    };
    let Ok(sdoc) = xdoc(arg) else {
        return;
    };
    // SAFETY: `sdoc` is the arena of `arg`'s Document, which `arg` keeps alive,
    // and `src` is its own node. No Ruby runs in the detaching below.
    unsafe {
        if (*sdoc).type_(src) == Some(NodeType::Fragment) {
            while let Some(c) = (*sdoc).first_child(src) {
                xml_remove(&mut *sdoc, c);
            }
        } else {
            xml_remove(&mut *sdoc, src);
        }
        xml_name_index_invalidate(&mut *sdoc);
    }
}

/// A DOCUMENT_FRAGMENT contributes its CHILDREN, not itself.
unsafe fn splice_fragment(
    xd: *mut XmlDoc,
    target: NodeId,
    frag: NodeId,
    doc_v: Value,
    op: Op,
) -> Result<Value, Error> {
    if op == Op::Replace {
        // SAFETY: the caller's arena; the primitive validates before linking.
        unsafe { xml_mut_check(xml_replace_with_fragment(&mut *xd, target, frag))? };
        return Ok(wrap(frag, doc_v));
    }
    let mut r = target; /* the moving insertion point, for AFTER */
    // SAFETY: the caller's arena; each call detaches c from frag.
    while let Some(c) = unsafe { (*xd).first_child(frag) } {
        let st = unsafe {
            match op {
                Op::Child => xml_insert_child(&mut *xd, target, c),
                Op::After => {
                    let s = xml_insert_after(&mut *xd, r, c);
                    r = c;
                    s
                }
                _ => xml_insert_before(&mut *xd, target, c),
            }
        };
        xml_mut_check(st)?;
    }
    Ok(wrap(frag, doc_v))
}

fn insert(ruby: &Ruby, this: XmlSelf, arg: Value, op: Op) -> Result<Value, Error> {
    let target = unwrap_mutable(this)?;
    let doc_v = this.document;
    let xd = this.doc();
    // SAFETY: the receiver's arena and the source node's, both live.
    let (node, adopt_from) = unsafe { incoming_node(ruby, xd, doc_v, arg)? };

    // SAFETY: `xd` is the receiver's arena, live for this call.
    if unsafe { (*xd).type_(node) } == Some(NodeType::Fragment) {
        // SAFETY: as above.
        let out = unsafe { splice_fragment(xd, target, node, doc_v, op)? };
        adopt_finish(adopt_from);
        return Ok(out);
    }

    // SAFETY: as above.
    let st = unsafe {
        match op {
            Op::Child => xml_insert_child(&mut *xd, target, node),
            Op::Before => xml_insert_before(&mut *xd, target, node),
            Op::After => xml_insert_after(&mut *xd, target, node),
            Op::Replace => xml_replace_node(&mut *xd, target, node),
        }
    };
    xml_mut_check(st)?;
    adopt_finish(adopt_from);
    Ok(wrap(node, doc_v))
}

pub fn add_child(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Child)
}
pub fn before(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Before)
}
pub fn after(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::After)
}
pub fn replace(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Replace)
}

/// `element << node` -> self.
pub fn lshift(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    insert(ruby, this, arg, Op::Child)?;
    Ok(rb_self)
}

/// `clone_node(deep = false)` -> a detached copy in the same document.
pub fn clone_node(this: XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());
    let mut out: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, live for this call.
    unsafe { xml_mut_check(xml_clone_node(&mut *this.doc(), this.id, deep, &mut out))? };
    Ok(xml_wrap_rel_value(this, out))
}

/* ------------------------------------------------------------------ */
/* document factories                                                 */
/* ------------------------------------------------------------------ */

/* WHATWG DOM element-name rules, for the loose escape hatch below. */

fn dom_name_forbidden(c: u8) -> bool {
    matches!(c, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' | b'/' | b'>')
}

fn dom_prefix_ok(p: &[u8]) -> bool {
    !p.is_empty() && !p.iter().copied().any(dom_name_forbidden)
}

fn dom_local_ok(p: &[u8]) -> bool {
    let Some(&first) = p.first() else {
        return false;
    };
    if first < 0x80 && !(first.is_ascii_alphabetic() || first == b':' || first == b'_') {
        return false;
    }
    !p.iter().copied().any(dom_name_forbidden)
}

/// Check that the three name pieces describe the same name.
fn dom_name_consistency(
    ruby: &Ruby,
    qv: &RubyText,
    pv: &RubyText,
    has_prefix: bool,
    lv: &RubyText,
) -> Result<(u32, u32, u32), Error> {
    /* SAFETY: the three views are the caller's, live for this call, and only
     * their bytes are compared - nothing here runs Ruby. */
    let (q, p, l) = unsafe { (qv.bytes(), pv.bytes(), lv.bytes()) };
    let arg_err = |msg: &str| Error::new(ruby.exception_arg_error(), msg.to_string());

    if !dom_local_ok(l) {
        return Err(arg_err("invalid DOM element local name"));
    }
    if !has_prefix {
        if q != l {
            return Err(arg_err(
                "qualified name must equal local name when prefix is nil",
            ));
        }
        return Ok((0, 0, q.len() as u32));
    }

    if !dom_prefix_ok(p) {
        return Err(arg_err("invalid DOM element prefix"));
    }
    if q.len() != p.len() + 1 + l.len()
        || &q[..p.len()] != p
        || q[p.len()] != b':'
        || &q[p.len() + 1..] != l
    {
        return Err(arg_err("qualified name must be prefix + ':' + local name"));
    }
    Ok((p.len() as u32, (p.len() + 1) as u32, l.len() as u32))
}

/// `create_element(name, content = nil, attributes = {})` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), magnus::RArray, (), (), ()>(args)?;
    let (name,) = a.required;
    let mut content = ruby.qnil().as_value();
    let mut attrs: Option<RHash> = None;
    for v in a.splat.into_iter() {
        if let Some(h) = RHash::from_value(v) {
            attrs = Some(h);
        } else if !v.is_nil() {
            content = v;
        }
    }

    let xd = xdoc(rb_self)?;
    let (nv, _) = verified(ruby, name, c"element name")?;
    let mut el: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the view is live for the call.
    let st = unsafe { xml_new_element(&mut *xd, nv.bytes(), &mut el) };
    xml_mut_check(st)?;

    if !content.is_nil() {
        let (tv, _) = verified(ruby, content, c"element content")?;
        // SAFETY: as above.
        let st = unsafe { xml_set_content(&mut *xd, el, tv.bytes()) };
        xml_mut_check(st)?;
    }
    let rb_el = wrap(el, rb_self);
    if let Some(h) = attrs {
        /* Keys and values are stringified - Nokogiri accepts symbol keys and
         * non-string values - then go through the normal validated setter. */
        let pairs: RArray = h.funcall("to_a", ())?;
        for pair in pairs.into_iter() {
            let entry = RArray::from_value(pair).expect("Hash#to_a yields pairs");
            let k: Value = entry.entry(0)?;
            let v: Value = entry.entry(1)?;
            /* `rb_el` was wrapped just above, so it converts. */
            let el_self = <XmlSelf as magnus::TryConvert>::try_convert(rb_el)?;
            aset(
                ruby,
                el_self,
                k.funcall("to_s", ())?,
                v.funcall("to_s", ())?,
            )?;
        }
    }
    Ok(rb_el)
}

/// `create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)`
/// -> Element.
pub fn create_loose_dom_element(
    ruby: &Ruby,
    rb_self: Value,
    qname: Value,
    prefix: Value,
    local: Value,
    ns: Value,
) -> Result<Value, Error> {
    let xd = xdoc(rb_self)?;
    let (qv, _) = verified(ruby, qname, c"qualified name")?;
    let (lv, _) = verified(ruby, local, c"local name")?;
    let has_prefix = !prefix.is_nil();
    let (pv, _) = verified_opt(ruby, prefix, c"prefix")?;
    let (nv, _) = verified_opt(ruby, ns, c"namespace URI")?;

    let (plen, loff, llen) = dom_name_consistency(ruby, &qv, &pv, has_prefix, &lv)?;
    let mut el: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the views are live for the call.
    let st = unsafe {
        xml_new_loose_dom_element(&mut *xd, qv.bytes(), plen, loff, llen, nv.bytes(), &mut el)
    };
    xml_mut_check(st)?;
    Ok(wrap(el, rb_self))
}

/// `create_document_type(name, public_id = "", system_id = "")` -> DocumentType.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a =
        magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
            args,
        )?;
    let name = a.required.0;
    let nil = ruby.qnil().as_value();
    let pub_v = a.optional.0.unwrap_or(nil);
    let sys_v = a.optional.1.unwrap_or(nil);

    let xd = xdoc(rb_self)?;
    let (nv, _) = verified(ruby, name, c"doctype name")?;
    let (pv, pl) = verified_opt(ruby, pub_v, c"doctype public id")?;
    let (sv, sl) = verified_opt(ruby, sys_v, c"doctype system id")?;
    /* An empty id is absent (NULL), matching the HTML factory and Nokogiri. */
    let mut dt: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the views are live for the call.
    let st = unsafe {
        xml_new_document_type(
            &mut *xd,
            nv.bytes(),
            (pl != 0).then_some(pv.bytes()),
            (sl != 0).then_some(sv.bytes()),
            &mut dt,
        )
    };
    xml_mut_check(st)?;
    Ok(wrap(dt, rb_self))
}

/// The shared body of the leaf-data factories.
fn create_chardata(
    ruby: &Ruby,
    rb_self: Value,
    text: Value,
    type_: NodeType,
    what: &core::ffi::CStr,
) -> Result<Value, Error> {
    let xd = xdoc(rb_self)?;
    let (tv, _) = verified(ruby, text, what)?;
    let mut n: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the view is the caller's for the copy.
    let st = unsafe { xml_new_chardata(&mut *xd, type_, tv.bytes(), &mut n) };
    xml_mut_check(st)?;
    Ok(wrap(n, rb_self))
}

pub fn create_text_node(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, NodeType::Text, c"text content")
}
pub fn create_comment(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, NodeType::Comment, c"comment content")
}
pub fn create_cdata(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, NodeType::CData, c"CDATA content")
}

pub fn create_pi(ruby: &Ruby, rb_self: Value, target: Value, data: Value) -> Result<Value, Error> {
    let xd = xdoc(rb_self)?;
    let (tg, _) = verified(ruby, target, c"PI target")?;
    let (dt, _) = verified(ruby, data, c"PI data")?;
    let mut pi: NodeId = NodeId::INVALID;
    // SAFETY: the receiver's arena, and the views are live for the call.
    let st = unsafe { xml_new_pi(&mut *xd, tg.bytes(), dt.bytes(), &mut pi) };
    xml_mut_check(st)?;
    Ok(wrap(pi, rb_self))
}

/// `Document#import_node(node, deep = false)` - the DOM's importNode.
pub fn import_node(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let node_v = a.required.0;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());

    let xd = crate::bridge::lexbor::doc_parsed(rb_self)
        .map(|p| unsafe { crate::bridge::lexbor::parsed_xml_doc(p) })
        .unwrap_or(core::ptr::null_mut());
    let mut copy: NodeId = NodeId::INVALID;
    match node_kind(node_v.as_raw()) {
        KIND_XML => {
            let src_doc = xdoc(node_v)?;
            if src_doc == xd {
                /* Same arena: the single-`&mut` clone path. */
                // SAFETY: the target arena, which is the source here.
                xml_mut_check(unsafe { xml_clone_node(&mut *xd, unwrap(node_v)?, deep, &mut copy) })?
            } else {
                // SAFETY: two distinct live arenas.
                xml_mut_check(unsafe {
                    xml_copy_node(&mut *xd, &*src_doc, unwrap(node_v)?, deep, &mut copy)
                })?
            }
        }
        KIND_HTML => {
            // SAFETY: the target arena, and the HTML source node.
            xml_mut_check(unsafe {
                cross_html_to_xml(xd, html_node_unwrap(node_v)?, deep, &mut copy)
            })?
        }
        _ => {
            return Err(Error::new(
                ruby.exception_type_error(),
                "import_node expects a Makiri node",
            ))
        }
    }
    Ok(wrap(copy, rb_self))
}
