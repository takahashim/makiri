//! Changing an XML tree: the in-place edits, the insertion verbs, and the
//! document factories (glue/ruby_xml_node.c).
//!
//! This is only the Ruby boundary. The rules - name well-formedness, the XML
//! character class, namespace resolution, what may be a child of what - live in
//! the Ruby-free primitives of `xml/mkr_xml_mutate.c`; this layer coerces and
//! verifies arguments through the bridge and maps the resulting status to a Ruby
//! exception.
//!
//! **Detach, never destroy.** A removed node is unlinked, not freed, so a live
//! Ruby wrapper for it stays valid. The arena owns the memory and outlives every
//! wrapper through the Document.
//!
//! Unlike the HTML side there is no attr or text index to invalidate, but there
//! is an element-name index, and [`unwrap_mutable`] is the single choke point
//! every mutator goes through - so dropping it cannot be forgotten in one path.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};

use super::abi::*;
use super::{node_document, unwrap, wrap};
use crate::glue::abi::{doc_parsed, html_node_unwrap, parsed_xml_doc};
use crate::init::CLASS_NODE;

/// `NodeKind`.
const KIND_HTML: core::ffi::c_int = 1;
const KIND_XML: core::ffi::c_int = 2;

pub use crate::lexbor::adapter::cross_import::cross_html_to_xml;
pub use crate::glue::node::node_kind;
pub use crate::xml::api::xml_clone_node;
pub use crate::xml::api::xml_copy_node;
pub use crate::xml::api::xml_import_subtree;
pub use crate::xml::api::xml_insert_after;
pub use crate::xml::api::xml_insert_before;
pub use crate::xml::api::xml_insert_child;
pub use crate::xml::api::xml_name_index_invalidate;
pub use crate::xml::api::xml_new_chardata;
pub use crate::xml::api::xml_new_document_type;
pub use crate::xml::api::xml_new_element;
pub use crate::xml::api::xml_new_loose_dom_element;
pub use crate::xml::api::xml_new_pi;
pub use crate::xml::api::xml_remove;
pub use crate::xml::api::xml_remove_attribute;
pub use crate::xml::api::xml_remove_attribute_ns;
pub use crate::xml::api::xml_rename;
pub use crate::xml::api::xml_replace_node;
pub use crate::xml::api::xml_replace_with_fragment;
pub use crate::xml::api::xml_set_attribute;
pub use crate::xml::api::xml_set_attribute_ns;
pub use crate::xml::api::xml_set_content;

/// The exception for a non-OK mutation status; [`MutStatus::Ok`] is `Ok`.
///
/// The one place a mutation failure becomes an exception, so every entry point
/// that can produce a [`MutStatus`] routes through it: the node mutators here
/// and `glue/doc.rs`. It hands the error back rather than raising, so the
/// caller's frames unwind normally and nothing they own is skipped.
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
        /* No C caller, so a null/stale document handle reaching a mutator is a
         * Rust-side invariant break, not a user error. */
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
    Ok(unsafe { parsed_xml_doc(crate::glue::doc::doc_parsed_known(document)) } as *mut XmlDoc)
}

/// A byte length as the arena's `uint32`, or an error.
fn u32_len(ruby: &Ruby, len: usize) -> Result<u32, Error> {
    u32::try_from(len).map_err(|_| {
        let _ = ruby;
        Error::new(error_class(), "string too long for an XML node (max 4 GiB)")
    })
}

/// Unwrap for mutation.
///
/// A frozen node is immutable, so this raises FrozenError rather than editing
/// it, the same contract HTML nodes have. It is also the single mutation choke
/// point: every mutator comes through here, and here is where the cached
/// element-name index is dropped so the next query rebuilds it.
///
/// A document an XPath handler is being evaluated over refuses to change, so
/// that check comes before the index is dropped.
fn unwrap_mutable(this: super::XmlSelf) -> Result<NodeId, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    crate::glue::doc::ensure_document_mutable(this.document)?;
    // SAFETY: the receiver's own arena, which the receiver keeps alive, and
    // nothing else holds a borrow of it here.
    unsafe { xml_name_index_invalidate(&mut *this.doc()) };
    Ok(this.id)
}

/// Verify a String argument and hand back its bytes plus the length the arena
/// wants. The `Value` is returned so the caller keeps it rooted: the pointer
/// borrows it.
fn verified(ruby: &Ruby, v: Value, what: &core::ffi::CStr) -> Result<(RubyText, u32), Error> {
    let t = ruby_verified_text(v, what)?;
    let n = u32_len(ruby, t.len())?;
    Ok((t, n))
}

/// The same for an optional argument: nil is (NULL, 0), which every primitive
/// reads as "absent".
fn verified_opt(ruby: &Ruby, v: Value, what: &core::ffi::CStr) -> Result<(RubyText, u32), Error> {
    if v.is_nil() {
        return Ok((RubyText::absent(), 0));
    }
    verified(ruby, v, what)
}

/* ------------------------------------------------------------------ */
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self. Detaches from the tree (or, for an attribute,
/// from its owner); the node stays usable.
pub fn remove(this: super::XmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        if is_a(rb_self, &CLASS_XML_DOCUMENT) {
            return Err(Error::new(error_class(), "cannot remove the document node"));
        }
        let n = unwrap_mutable(this)?;
        xml_remove(&mut *this.doc(), n); /* detach + refresh the root/doctype cache */
        Ok(rb_self)
    }
}

/// The element behind `rb_self`, or an error naming what was attempted.
fn element_for(this: super::XmlSelf) -> Result<NodeId, Error> {
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

/// `element[name] = value` -> value. Adds or replaces the attribute.
pub fn aset(ruby: &Ruby, this: super::XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    unsafe {
        let n = element_for(this)?;
        let (nv, _) = verified(ruby, name, c"attribute name")?;
        let (vv, _) = verified(ruby, val, c"attribute value")?;
        let mut out = NodeId::INVALID;
        let st = xml_set_attribute(&mut *this.doc(), n, nv.bytes(), vv.bytes(), &mut out);
        xml_mut_check(st)?;
        Ok(val)
    }
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
///
/// Stores the attribute keyed on (explicit namespace, local name) - the DOM key -
/// with its qualified name case-preserved. A null or empty namespace is the null
/// namespace, and xmlns declarations pass through as ordinary attributes in the
/// xmlns namespace.
pub fn set_attribute_ns(
    ruby: &Ruby,
    this: super::XmlSelf,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    unsafe {
        let n = element_for(this)?;
        let (qv, _) = verified(ruby, qname, c"attribute qualified name")?;
        let (vv, _) = verified(ruby, val, c"attribute value")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
        let mut out = NodeId::INVALID;
        let st = xml_set_attribute_ns(
            &mut *this.doc(),
            n,
            nv.bytes(),
            qv.bytes(),
            vv.bytes(),
            &mut out,
        );
        xml_mut_check(st)?;
        Ok(val)
    }
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: super::XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let n = unwrap_mutable(this)?;
        if (*this.doc()).type_(n) != Some(NodeType::Element) {
            return Ok(rb_self);
        }
        let (lv, _) = verified(ruby, local, c"attribute local name")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
        xml_remove_attribute_ns(&mut *this.doc(), n, nv.bytes(), lv.bytes());
        Ok(rb_self)
    }
}

/// `element.delete(name)` / `#remove_attribute` -> self. A no-op when absent.
pub fn delete(ruby: &Ruby, this: super::XmlSelf, name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let n = unwrap_mutable(this)?;
        if (*this.doc()).type_(n) != Some(NodeType::Element) {
            return Ok(rb_self);
        }
        let (nv, _) = verified(ruby, name, c"attribute name")?;
        xml_remove_attribute(&mut *this.doc(), n, nv.bytes());
        Ok(rb_self)
    }
}

/// `node.content = text` -> text. For an element, replaces its children with one
/// text node (stored verbatim, escaped on serialization); for a text, CDATA,
/// comment or PI leaf, sets its data.
pub fn set_content(ruby: &Ruby, this: super::XmlSelf, text: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(this)?;
        let (tv, _) = verified(ruby, text, c"node content")?;
        let st = xml_set_content(&mut *this.doc(), n, tv.bytes());
        xml_mut_check(st)?;
        Ok(text)
    }
}

/// `node.name = new_name` -> new_name. Renames an element or attribute in place,
/// preserving identity and tree position; the namespace is re-resolved against
/// the node's in-scope declarations.
pub fn set_name(ruby: &Ruby, this: super::XmlSelf, name: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(this)?;
        let (nv, _) = verified(ruby, name, c"node name")?;
        let st = xml_rename(&mut *this.doc(), n, nv.bytes());
        xml_mut_check(st)?;
        Ok(name)
    }
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
///
/// A same-document node is returned as-is, which makes the insert a move. A node
/// from another document cannot be relinked - the arenas own their own nodes -
/// so it is copied here, and taken out of the document it came from only once
/// the insert has actually succeeded, so the operation reads as the move the DOM
/// says it is. The second return value is that source node, or nil.
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
    /* Adopting takes the node out of the document it came from, so that
     * document changes too - refuse before anything is copied. */
    crate::glue::doc::ensure_document_mutable(node_document(arg)?)?;
    let mut copy: NodeId = NodeId::INVALID;
    let src_doc = xdoc(arg)?;
    xml_mut_check(xml_import_subtree(&mut *xd, &*src_doc, src, &mut copy))?;
    Ok((copy, arg))
}

/// Finish the adoption by emptying the node out of its old document.
///
/// Only called after the insert succeeded, so a rejected one leaves the source
/// document alone. A fragment is emptied rather than detached: it contributed
/// its children, and the DOM leaves a spliced fragment empty.
fn adopt_finish(arg: Value) {
    if arg.is_nil() {
        return;
    }
    /* The adopt step already unwrapped `arg`, so this cannot fail. */
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

/// A DOCUMENT_FRAGMENT contributes its CHILDREN, not itself, like Nokogiri and
/// the DOM: they are spliced in place of the fragment, in order, leaving it
/// empty. Each child is inserted relative to `target` per `op`, resolving its
/// namespaces against the new context as a single node would; for AFTER the
/// insertion point advances so the children keep their order.
unsafe fn splice_fragment(
    xd: *mut XmlDoc,
    target: NodeId,
    frag: NodeId,
    doc_v: Value,
    op: Op,
) -> Result<Value, Error> {
    if op == Op::Replace {
        /* Whole-fragment replace is an engine primitive: it validates the
         * fragment before touching a link and keeps `target` until every child
         * is spliced in, so a rejected replace never destroys what it replaced. */
        xml_mut_check(xml_replace_with_fragment(&mut *xd, target, frag))?;
        return Ok(wrap(frag, doc_v));
    }
    let mut r = target; /* the moving insertion point, for AFTER */
    while let Some(c) = (*xd).first_child(frag) {
        /* each insert detaches c from frag */
        let st = match op {
            Op::Child => xml_insert_child(&mut *xd, target, c),
            Op::After => {
                let s = xml_insert_after(&mut *xd, r, c);
                r = c;
                s
            }
            _ => xml_insert_before(&mut *xd, target, c),
        };
        xml_mut_check(st)?;
    }
    Ok(wrap(frag, doc_v))
}

fn insert(ruby: &Ruby, this: super::XmlSelf, arg: Value, op: Op) -> Result<Value, Error> {
    unsafe {
        let target = unwrap_mutable(this)?;
        let doc_v = this.document;
        let xd = this.doc();
        let (node, adopt_from) = incoming_node(ruby, xd, doc_v, arg)?;

        if (*xd).type_(node) == Some(NodeType::Fragment) {
            let out = splice_fragment(xd, target, node, doc_v, op)?;
            adopt_finish(adopt_from);
            return Ok(out);
        }

        let st = match op {
            Op::Child => xml_insert_child(&mut *xd, target, node),
            Op::Before => xml_insert_before(&mut *xd, target, node),
            Op::After => xml_insert_after(&mut *xd, target, node),
            Op::Replace => xml_replace_node(&mut *xd, target, node),
        };
        xml_mut_check(st)?;
        adopt_finish(adopt_from);
        Ok(wrap(node, doc_v))
    }
}

pub fn add_child(ruby: &Ruby, this: super::XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Child)
}
pub fn before(ruby: &Ruby, this: super::XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Before)
}
pub fn after(ruby: &Ruby, this: super::XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::After)
}
pub fn replace(ruby: &Ruby, this: super::XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Op::Replace)
}

/// `element << node` -> self. Nokogiri's `<<` appends and returns the receiver.
pub fn lshift(ruby: &Ruby, this: super::XmlSelf, arg: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    insert(ruby, this, arg, Op::Child)?;
    Ok(rb_self)
}

/// `clone_node(deep = false)` -> a detached copy in the same document, with the
/// element/attribute name case, the namespaces and the CDATA node type
/// preserved. Backs `#dup` / `#clone` and the DOM's cloneNode.
pub fn clone_node(this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());
    unsafe {
        let mut out: NodeId = NodeId::INVALID;
        xml_mut_check(xml_clone_node(&mut *this.doc(), this.id, deep, &mut out))?;
        Ok(super::xml_wrap_rel_value(this, out))
    }
}

/* ------------------------------------------------------------------ */
/* document factories                                                 */
/* ------------------------------------------------------------------ */

/* WHATWG DOM element-name rules, for the loose escape hatch below. They are
 * deliberately laxer than XML's QName production: the DOM only forbids the bytes
 * that would break parsing back out. */

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

/// Check that the three name pieces describe the same name, and build the split
/// form the engine takes. Fails closed rather than storing a name whose parts
/// disagree.
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
///
/// Nokogiri-style trailing arguments: a Hash sets attributes, any other non-nil
/// argument is the element's text content.
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

    unsafe {
        let xd = xdoc(rb_self)?;
        let (nv, _) = verified(ruby, name, c"element name")?;
        let mut el: NodeId = NodeId::INVALID;
        let st = xml_new_element(&mut *xd, nv.bytes(), &mut el);
        xml_mut_check(st)?;

        if !content.is_nil() {
            let (tv, _) = verified(ruby, content, c"element content")?;
            let st = xml_set_content(&mut *xd, el, tv.bytes());
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
                let el_self = <super::XmlSelf as magnus::TryConvert>::try_convert(rb_el)?;
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
}

/// `create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)`
/// -> Element.
///
/// An internal browser-DOM interop escape hatch: an XML-backed element whose name
/// follows WHATWG DOM rules rather than XML QName rules. The result is
/// deliberately not XML-serializable.
pub fn create_loose_dom_element(
    ruby: &Ruby,
    rb_self: Value,
    qname: Value,
    prefix: Value,
    local: Value,
    ns: Value,
) -> Result<Value, Error> {
    unsafe {
        let xd = xdoc(rb_self)?;
        let (qv, _) = verified(ruby, qname, c"qualified name")?;
        let (lv, _) = verified(ruby, local, c"local name")?;
        let has_prefix = !prefix.is_nil();
        let (pv, _) = verified_opt(ruby, prefix, c"prefix")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace URI")?;

        let (plen, loff, llen) = dom_name_consistency(ruby, &qv, &pv, has_prefix, &lv)?;
        let mut el: NodeId = NodeId::INVALID;
        let st =
            xml_new_loose_dom_element(&mut *xd, qv.bytes(), plen, loff, llen, nv.bytes(), &mut el);
        xml_mut_check(st)?;
        Ok(wrap(el, rb_self))
    }
}

/// `create_document_type(name, public_id = "", system_id = "")` -> DocumentType.
///
/// The DOM's createDocumentType: a detached DocumentType owned by this document,
/// to be placed before the root with `add_child` / `add_previous_sibling` (the
/// placement guards keep it document-level, pre-root and single). An omitted or
/// empty identifier is absent; an invalid name fails closed.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
        args,
    )?;
    let name = a.required.0;
    let nil = ruby.qnil().as_value();
    let pub_v = a.optional.0.unwrap_or(nil);
    let sys_v = a.optional.1.unwrap_or(nil);

    unsafe {
        let xd = xdoc(rb_self)?;
        let (nv, _) = verified(ruby, name, c"doctype name")?;
        let (pv, pl) = verified_opt(ruby, pub_v, c"doctype public id")?;
        let (sv, sl) = verified_opt(ruby, sys_v, c"doctype system id")?;
        /* An empty id is absent (NULL), matching the HTML factory and Nokogiri. */
        let mut dt: NodeId = NodeId::INVALID;
        let st = xml_new_document_type(
            &mut *xd,
            nv.bytes(),
            (pl != 0).then_some(pv.bytes()),
            (sl != 0).then_some(sv.bytes()),
            &mut dt,
        );
        xml_mut_check(st)?;
        Ok(wrap(dt, rb_self))
    }
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
    /* SAFETY: the receiver's own arena, and `tv`'s bytes, which it holds rooted
     * for the copy the arena makes. */
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
    unsafe {
        let xd = xdoc(rb_self)?;
        let (tg, _) = verified(ruby, target, c"PI target")?;
        let (dt, _) = verified(ruby, data, c"PI data")?;
        let mut pi: NodeId = NodeId::INVALID;
        let st = xml_new_pi(&mut *xd, tg.bytes(), dt.bytes(), &mut pi);
        xml_mut_check(st)?;
        Ok(wrap(pi, rb_self))
    }
}

/// `Document#import_node(node, deep = false)` - the DOM's importNode.
///
/// An XML node is copied into this document's arena (namespaces re-resolved when
/// it is later linked); an HTML node is TRANSLATED across representations. The
/// result is detached and owned by this document, the source is untouched, and a
/// failure returns no partial node.
pub fn import_node(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let node_v = a.required.0;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());

    unsafe {
        let xd = parsed_xml_doc(doc_parsed(rb_self)?) as *mut XmlDoc;
        let mut copy: NodeId = NodeId::INVALID;
        match node_kind(node_v.as_raw()) {
            KIND_XML => {
                let src_doc = xdoc(node_v)?;
                if src_doc == xd {
                    /* Same arena: the single-`&mut` clone path. Going through
                     * `xml_copy_node` would hand `&mut *xd` and `&*src_doc`
                     * as the same document (aliasing UB). */
                    xml_mut_check(xml_clone_node(&mut *xd, unwrap(node_v)?, deep, &mut copy))?
                } else {
                    xml_mut_check(xml_copy_node(
                        &mut *xd,
                        &*src_doc,
                        unwrap(node_v)?,
                        deep,
                        &mut copy,
                    ))?
                }
            }
            KIND_HTML => xml_mut_check(cross_html_to_xml(
                xd,
                html_node_unwrap(node_v)?,
                deep,
                &mut copy,
            ))?,
            _ => {
                return Err(Error::new(
                    ruby.exception_type_error(),
                    "import_node expects a Makiri node",
                ))
            }
        }
        Ok(wrap(copy, rb_self))
    }
}
