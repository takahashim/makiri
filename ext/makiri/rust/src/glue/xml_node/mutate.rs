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

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};
use rb_sys::VALUE;

use super::abi::*;
use super::{node_document, unwrap, wrap};
use crate::glue::abi::{mkr_cNode, mkr_doc_parsed, mkr_html_node_unwrap, mkr_parsed_xml_doc};

/// `mkr_node_kind_t`.
const KIND_HTML: core::ffi::c_int = 1;
const KIND_XML: core::ffi::c_int = 2;

pub use crate::dom_adapter::cross_import::mkr_cross_html_to_xml;
pub use crate::glue::node::mkr_node_kind;
pub use crate::xml::api::mkr_xml_clone_node;
pub use crate::xml::api::mkr_xml_copy_node;
pub use crate::xml::api::mkr_xml_import_subtree;
pub use crate::xml::api::mkr_xml_insert_after;
pub use crate::xml::api::mkr_xml_insert_before;
pub use crate::xml::api::mkr_xml_insert_child;
pub use crate::xml::api::mkr_xml_name_index_invalidate;
pub use crate::xml::api::mkr_xml_new_chardata;
pub use crate::xml::api::mkr_xml_new_document_type;
pub use crate::xml::api::mkr_xml_new_element;
pub use crate::xml::api::mkr_xml_new_loose_dom_element;
pub use crate::xml::api::mkr_xml_new_pi;
pub use crate::xml::api::mkr_xml_remove;
pub use crate::xml::api::mkr_xml_remove_attribute;
pub use crate::xml::api::mkr_xml_remove_attribute_ns;
pub use crate::xml::api::mkr_xml_rename;
pub use crate::xml::api::mkr_xml_replace_node;
pub use crate::xml::api::mkr_xml_replace_with_fragment;
pub use crate::xml::api::mkr_xml_set_attribute;
pub use crate::xml::api::mkr_xml_set_attribute_ns;
pub use crate::xml::api::mkr_xml_set_content;

extern "C" {

    static rb_eArgError: VALUE;
}

/// Raise for a non-OK mutation status; [`MutStatus::Ok`] returns.
///
/// The one place a mutation failure becomes an exception, so every entry point
/// that can produce a [`MutStatus`] routes through it: the node mutators here,
/// `glue/doc.rs`, and `dom_adapter/cross_import.rs`. (Dropping the C file that
/// used to define it without providing this is what turned the first build of
/// this port into a crash rather than a link error - on macOS an unresolved
/// symbol becomes a NULL jump at runtime.)
///
/// # Safety
/// Raises, so no Rust destructor may be live at the call. Every caller here
/// passes only `Copy` locals.
pub unsafe fn mkr_xml_mut_check(st: MutStatus) {
    let (exc, msg) = match st {
        MutStatus::Ok => return,
        MutStatus::Oom => (error_class().as_raw(), c"out of memory mutating XML"),
        MutStatus::BadName => (rb_eArgError, c"not a well-formed XML name"),
        MutStatus::BadChars => (
            error_class().as_raw(),
            c"value contains a character or sequence not permitted in XML",
        ),
        MutStatus::UnboundNs => (
            error_class().as_raw(),
            c"namespace prefix is not bound in this scope",
        ),
        MutStatus::Type => (
            error_class().as_raw(),
            c"operation unsupported for this node type",
        ),
        MutStatus::Cycle => (
            error_class().as_raw(),
            c"cannot insert a node into its own subtree",
        ),
        MutStatus::Hierarchy => (
            error_class().as_raw(),
            c"invalid placement (an attribute/document node cannot be a tree child, a document \
allows a single root element, and a sibling target must have a parent)",
        ),
        MutStatus::BadNsDecl => (
            error_class().as_raw(),
            c"cannot bind a namespace prefix to the empty namespace",
        ),
        /* No C caller, so a null/stale document handle reaching a mutator is a
         * Rust-side invariant break, not a user error. */
        MutStatus::Internal => (
            error_class().as_raw(),
            c"internal error mutating XML (no document)",
        ),
    };
    crate::glue::abi::rb_raise(exc, c"%s".as_ptr(), msg.as_ptr())
}

/* ------------------------------------------------------------------ */
/* helpers                                                            */
/* ------------------------------------------------------------------ */

/// The arena behind a node's document.
unsafe fn xdoc(rb_self: Value) -> *mut XmlDoc {
    mkr_parsed_xml_doc(mkr_doc_parsed(node_document(rb_self).as_raw())) as *mut XmlDoc
}

/// A byte length as the arena's `uint32`, or an error.
fn u32_len(ruby: &Ruby, len: usize) -> Result<u32, Error> {
    u32::try_from(len).map_err(|_| {
        let _ = ruby;
        Error::new(
            unsafe { error_class() },
            "string too long for an XML node (max 4 GiB)",
        )
    })
}

/// Unwrap for mutation.
///
/// A frozen node is immutable, so this raises FrozenError rather than editing
/// it, the same contract HTML nodes have. It is also the single mutation choke
/// point: every mutator comes through here, and here is where the cached
/// element-name index is dropped so the next query rebuilds it.
unsafe fn unwrap_mutable(rb_self: Value) -> NodeId {
    rb_sys::rb_check_frozen(rb_self.as_raw());
    mkr_xml_name_index_invalidate(&mut *xdoc(rb_self));
    unwrap(rb_self)
}

/// Verify a String argument and hand back its bytes plus the length the arena
/// wants. The `Value` is returned so the caller keeps it rooted: the pointer
/// borrows it.
unsafe fn verified(
    ruby: &Ruby,
    v: Value,
    what: &core::ffi::CStr,
) -> Result<(BorrowedText, u32), Error> {
    let t = mkr_ruby_verified_text(v.as_raw(), what.as_ptr());
    let n = u32_len(ruby, t.len)?;
    Ok((t, n))
}

/// The same for an optional argument: nil is (NULL, 0), which every primitive
/// reads as "absent".
unsafe fn verified_opt(
    ruby: &Ruby,
    v: Value,
    what: &core::ffi::CStr,
) -> Result<(BorrowedText, u32), Error> {
    if v.is_nil() {
        return Ok((
            BorrowedText {
                value: 0,
                ptr: core::ptr::null(),
                len: 0,
            },
            0,
        ));
    }
    verified(ruby, v, what)
}

/* ------------------------------------------------------------------ */
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self. Detaches from the tree (or, for an attribute,
/// from its owner); the node stays usable.
pub fn remove(rb_self: Value) -> Result<Value, Error> {
    unsafe {
        if is_a(rb_self, mkr_cXmlDocument) {
            return Err(Error::new(error_class(), "cannot remove the document node"));
        }
        let n = unwrap_mutable(rb_self);
        mkr_xml_remove(&mut *xdoc(rb_self), n); /* detach + refresh the root/doctype cache */
        Ok(rb_self)
    }
}

/// The element behind `rb_self`, or an error naming what was attempted.
unsafe fn element_for(rb_self: Value) -> Result<NodeId, Error> {
    let n = unwrap_mutable(rb_self);
    if (*xdoc(rb_self)).type_(n) != Some(NodeType::Element) {
        return Err(Error::new(
            error_class(),
            "cannot set an attribute on a non-element node",
        ));
    }
    Ok(n)
}

/// `element[name] = value` -> value. Adds or replaces the attribute.
pub fn aset(ruby: &Ruby, rb_self: Value, name: Value, val: Value) -> Result<Value, Error> {
    unsafe {
        let n = element_for(rb_self)?;
        let (nv, _) = verified(ruby, name, c"attribute name")?;
        let (vv, _) = verified(ruby, val, c"attribute value")?;
        let mut out = NodeId::INVALID;
        let st = mkr_xml_set_attribute(&mut *xdoc(rb_self), n, nv.bytes(), vv.bytes(), &mut out);
        /* Keep both Strings reachable until the arena has copied their bytes. */
        core::hint::black_box((name, val));
        mkr_xml_mut_check(st);
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
    rb_self: Value,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    unsafe {
        let n = element_for(rb_self)?;
        let (qv, _) = verified(ruby, qname, c"attribute qualified name")?;
        let (vv, _) = verified(ruby, val, c"attribute value")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
        let mut out = NodeId::INVALID;
        let st = mkr_xml_set_attribute_ns(
            &mut *xdoc(rb_self),
            n,
            nv.bytes(),
            qv.bytes(),
            vv.bytes(),
            &mut out,
        );
        core::hint::black_box((qname, val, ns));
        mkr_xml_mut_check(st);
        Ok(val)
    }
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    rb_self: Value,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(rb_self);
        if (*xdoc(rb_self)).type_(n) != Some(NodeType::Element) {
            return Ok(rb_self);
        }
        let (lv, _) = verified(ruby, local, c"attribute local name")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace")?;
        mkr_xml_remove_attribute_ns(&mut *xdoc(rb_self), n, nv.bytes(), lv.bytes());
        core::hint::black_box((local, ns));
        Ok(rb_self)
    }
}

/// `element.delete(name)` / `#remove_attribute` -> self. A no-op when absent.
pub fn delete(ruby: &Ruby, rb_self: Value, name: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(rb_self);
        if (*xdoc(rb_self)).type_(n) != Some(NodeType::Element) {
            return Ok(rb_self);
        }
        let (nv, _) = verified(ruby, name, c"attribute name")?;
        mkr_xml_remove_attribute(&mut *xdoc(rb_self), n, nv.bytes());
        core::hint::black_box(name);
        Ok(rb_self)
    }
}

/// `node.content = text` -> text. For an element, replaces its children with one
/// text node (stored verbatim, escaped on serialization); for a text, CDATA,
/// comment or PI leaf, sets its data.
pub fn set_content(ruby: &Ruby, rb_self: Value, text: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(rb_self);
        let (tv, _) = verified(ruby, text, c"node content")?;
        let st = mkr_xml_set_content(&mut *xdoc(rb_self), n, tv.bytes());
        core::hint::black_box(text);
        mkr_xml_mut_check(st);
        Ok(text)
    }
}

/// `node.name = new_name` -> new_name. Renames an element or attribute in place,
/// preserving identity and tree position; the namespace is re-resolved against
/// the node's in-scope declarations.
pub fn set_name(ruby: &Ruby, rb_self: Value, name: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap_mutable(rb_self);
        let (nv, _) = verified(ruby, name, c"node name")?;
        let st = mkr_xml_rename(&mut *xdoc(rb_self), n, nv.bytes());
        core::hint::black_box(name);
        mkr_xml_mut_check(st);
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
    if !is_a(arg, mkr_cNode) || !is_a(node_document(arg), mkr_cXmlDocument) {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::XML node (NodeSet / String arguments are a later phase)",
        ));
    }
    let src = unwrap(arg);
    if node_document(arg).as_raw() == target_doc.as_raw() {
        return Ok((src, ruby.qnil().as_value())); /* same arena -> move */
    }
    let mut copy: NodeId = NodeId::INVALID;
    let src_doc = xdoc(arg);
    mkr_xml_mut_check(mkr_xml_import_subtree(&mut *xd, &*src_doc, src, &mut copy));
    Ok((copy, arg))
}

/// Finish the adoption by emptying the node out of its old document.
///
/// Only called after the insert succeeded, so a rejected one leaves the source
/// document alone. A fragment is emptied rather than detached: it contributed
/// its children, and the DOM leaves a spliced fragment empty.
unsafe fn adopt_finish(arg: Value) {
    if arg.is_nil() {
        return;
    }
    let src = unwrap(arg);
    let sdoc = xdoc(arg);
    if (*sdoc).type_(src) == Some(NodeType::Fragment) {
        while let Some(c) = (*sdoc).first_child(src) {
            mkr_xml_remove(&mut *sdoc, c);
        }
    } else {
        mkr_xml_remove(&mut *sdoc, src);
    }
    mkr_xml_name_index_invalidate(&mut *sdoc);
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
) -> Value {
    if op == Op::Replace {
        /* Whole-fragment replace is an engine primitive: it validates the
         * fragment before touching a link and keeps `target` until every child
         * is spliced in, so a rejected replace never destroys what it replaced. */
        mkr_xml_mut_check(mkr_xml_replace_with_fragment(&mut *xd, target, frag));
        return wrap(frag, doc_v);
    }
    let mut r = target; /* the moving insertion point, for AFTER */
    while let Some(c) = (*xd).first_child(frag) {
        /* each insert detaches c from frag */
        let st = match op {
            Op::Child => mkr_xml_insert_child(&mut *xd, target, c),
            Op::After => {
                let s = mkr_xml_insert_after(&mut *xd, r, c);
                r = c;
                s
            }
            _ => mkr_xml_insert_before(&mut *xd, target, c),
        };
        mkr_xml_mut_check(st);
    }
    wrap(frag, doc_v)
}

fn insert(ruby: &Ruby, rb_self: Value, arg: Value, op: Op) -> Result<Value, Error> {
    unsafe {
        let target = unwrap_mutable(rb_self);
        let doc_v = node_document(rb_self);
        let xd = xdoc(rb_self);
        let (node, adopt_from) = incoming_node(ruby, xd, doc_v, arg)?;

        if (*xd).type_(node) == Some(NodeType::Fragment) {
            let out = splice_fragment(xd, target, node, doc_v, op);
            adopt_finish(adopt_from);
            return Ok(out);
        }

        let st = match op {
            Op::Child => mkr_xml_insert_child(&mut *xd, target, node),
            Op::Before => mkr_xml_insert_before(&mut *xd, target, node),
            Op::After => mkr_xml_insert_after(&mut *xd, target, node),
            Op::Replace => mkr_xml_replace_node(&mut *xd, target, node),
        };
        mkr_xml_mut_check(st);
        adopt_finish(adopt_from);
        Ok(wrap(node, doc_v))
    }
}

pub fn add_child(ruby: &Ruby, rb_self: Value, arg: Value) -> Result<Value, Error> {
    insert(ruby, rb_self, arg, Op::Child)
}
pub fn before(ruby: &Ruby, rb_self: Value, arg: Value) -> Result<Value, Error> {
    insert(ruby, rb_self, arg, Op::Before)
}
pub fn after(ruby: &Ruby, rb_self: Value, arg: Value) -> Result<Value, Error> {
    insert(ruby, rb_self, arg, Op::After)
}
pub fn replace(ruby: &Ruby, rb_self: Value, arg: Value) -> Result<Value, Error> {
    insert(ruby, rb_self, arg, Op::Replace)
}

/// `element << node` -> self. Nokogiri's `<<` appends and returns the receiver.
pub fn lshift(ruby: &Ruby, rb_self: Value, arg: Value) -> Result<Value, Error> {
    insert(ruby, rb_self, arg, Op::Child)?;
    Ok(rb_self)
}

/// `clone_node(deep = false)` -> a detached copy in the same document, with the
/// element/attribute name case, the namespaces and the CDATA node type
/// preserved. Backs `#dup` / `#clone` and the DOM's cloneNode.
pub fn clone_node(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());
    unsafe {
        let mut out: NodeId = NodeId::INVALID;
        mkr_xml_mut_check(mkr_xml_clone_node(
            &mut *xdoc(rb_self),
            unwrap(rb_self),
            deep,
            &mut out,
        ));
        Ok(super::mkr_xml_wrap_rel_value(rb_self, out))
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
unsafe fn dom_name_consistency(
    ruby: &Ruby,
    qv: BorrowedText,
    pv: BorrowedText,
    has_prefix: bool,
    lv: BorrowedText,
) -> Result<(u32, u32, u32), Error> {
    let (q, p, l) = (qv.bytes(), pv.bytes(), lv.bytes());
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
        let xd = xdoc(rb_self);
        let (nv, _) = verified(ruby, name, c"element name")?;
        let mut el: NodeId = NodeId::INVALID;
        let st = mkr_xml_new_element(&mut *xd, nv.bytes(), &mut el);
        core::hint::black_box(name);
        mkr_xml_mut_check(st);

        if !content.is_nil() {
            let (tv, _) = verified(ruby, content, c"element content")?;
            let st = mkr_xml_set_content(&mut *xd, el, tv.bytes());
            core::hint::black_box(content);
            mkr_xml_mut_check(st);
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
                aset(ruby, rb_el, k.funcall("to_s", ())?, v.funcall("to_s", ())?)?;
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
        let xd = xdoc(rb_self);
        let (qv, _) = verified(ruby, qname, c"qualified name")?;
        let (lv, _) = verified(ruby, local, c"local name")?;
        let has_prefix = !prefix.is_nil();
        let (pv, _) = verified_opt(ruby, prefix, c"prefix")?;
        let (nv, _) = verified_opt(ruby, ns, c"namespace URI")?;

        let (plen, loff, llen) = dom_name_consistency(ruby, qv, pv, has_prefix, lv)?;
        let mut el: NodeId = NodeId::INVALID;
        let st = mkr_xml_new_loose_dom_element(
            &mut *xd,
            qv.bytes(),
            plen,
            loff,
            llen,
            nv.bytes(),
            &mut el,
        );
        core::hint::black_box((qname, local, prefix, ns));
        mkr_xml_mut_check(st);
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
        let xd = xdoc(rb_self);
        let (nv, _) = verified(ruby, name, c"doctype name")?;
        let (pv, pl) = verified_opt(ruby, pub_v, c"doctype public id")?;
        let (sv, sl) = verified_opt(ruby, sys_v, c"doctype system id")?;
        /* An empty id is absent (NULL), matching the HTML factory and Nokogiri. */
        let mut dt: NodeId = NodeId::INVALID;
        let st = mkr_xml_new_document_type(
            &mut *xd,
            nv.bytes(),
            (pl != 0).then_some(pv.bytes()),
            (sl != 0).then_some(sv.bytes()),
            &mut dt,
        );
        core::hint::black_box((name, pub_v, sys_v));
        mkr_xml_mut_check(st);
        Ok(wrap(dt, rb_self))
    }
}

/// The shared body of the leaf-data factories.
unsafe fn create_chardata(
    ruby: &Ruby,
    rb_self: Value,
    text: Value,
    type_: NodeType,
    what: &core::ffi::CStr,
) -> Result<Value, Error> {
    let xd = xdoc(rb_self);
    let (tv, _) = verified(ruby, text, what)?;
    let mut n: NodeId = NodeId::INVALID;
    let st = mkr_xml_new_chardata(&mut *xd, type_, tv.bytes(), &mut n);
    core::hint::black_box(text);
    mkr_xml_mut_check(st);
    Ok(wrap(n, rb_self))
}

pub fn create_text_node(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    unsafe { create_chardata(ruby, rb_self, t, NodeType::Text, c"text content") }
}
pub fn create_comment(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    unsafe { create_chardata(ruby, rb_self, t, NodeType::Comment, c"comment content") }
}
pub fn create_cdata(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    unsafe { create_chardata(ruby, rb_self, t, NodeType::CData, c"CDATA content") }
}

pub fn create_pi(ruby: &Ruby, rb_self: Value, target: Value, data: Value) -> Result<Value, Error> {
    unsafe {
        let xd = xdoc(rb_self);
        let (tg, _) = verified(ruby, target, c"PI target")?;
        let (dt, _) = verified(ruby, data, c"PI data")?;
        let mut pi: NodeId = NodeId::INVALID;
        let st = mkr_xml_new_pi(&mut *xd, tg.bytes(), dt.bytes(), &mut pi);
        core::hint::black_box((target, data));
        mkr_xml_mut_check(st);
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
        let xd = mkr_parsed_xml_doc(mkr_doc_parsed(rb_self.as_raw())) as *mut XmlDoc;
        let mut copy: NodeId = NodeId::INVALID;
        match mkr_node_kind(node_v.as_raw()) {
            KIND_XML => {
                let src_doc = xdoc(node_v);
                if src_doc == xd {
                    /* Same arena: the single-`&mut` clone path. Going through
                     * `mkr_xml_copy_node` would hand `&mut *xd` and `&*src_doc`
                     * as the same document (aliasing UB). */
                    mkr_xml_mut_check(mkr_xml_clone_node(
                        &mut *xd,
                        unwrap(node_v),
                        deep,
                        &mut copy,
                    ))
                } else {
                    mkr_xml_mut_check(mkr_xml_copy_node(
                        &mut *xd,
                        &*src_doc,
                        unwrap(node_v),
                        deep,
                        &mut copy,
                    ))
                }
            }
            KIND_HTML => mkr_xml_mut_check(mkr_cross_html_to_xml(
                xd,
                mkr_html_node_unwrap(node_v.as_raw()) as *mut _,
                deep,
                &mut copy,
            )),
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
