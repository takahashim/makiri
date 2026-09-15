//! The HTML node's mutators and the Document factories (glue/ruby_html_mutate.c).
//!
//! Thin wrappers over Lexbor's insert/remove/create, plus the safety checks
//! Lexbor itself omits: no cycles (a node cannot become a descendant of
//! itself), attribute nodes are not tree children, and the WHATWG doctype
//! ordering at the document node.
//!
//! # Adopt, never relink
//!
//! A node from another document is ADOPTED, as the DOM says appendChild does.
//! Lexbor's arenas own their nodes, so it cannot be relinked across them: it is
//! copied here ([`adopt_copy`]) and released there ([`adopt_release`]), and the
//! verb hands back the copy. The release happens only AFTER the insert has gone
//! through, so a refused insert leaves the source document alone.
//!
//! # Detach, never destroy
//!
//! The document arena owns all node memory and frees it wholesale, and live Ruby
//! wrappers may still point at a removed node, so `remove`/`unlink` only detach.
//!
//! # Every structural change drops the indexes
//!
//! The attr->owner + element-by-tag index and the text index are rebuilt on the
//! next query. [`invalidate`] is the one place that happens, and a mutator that
//! forgot to call it would serve a stale answer that looks entirely well-formed.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};

use super::ty;
use super::{node_document, unwrap, wrap};
use crate::glue::abi::{
    error_class, mkr_html_doc_unwrap, mkr_ruby_verified_text, LxbAttr, LxbDoc, LxbElement, LxbNode,
};
use crate::lexbor_abi as lxb;

const NS_UNDEF: usize = lxb::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
const STATUS_OK: u32 = lxb::lexbor_status_t_LXB_STATUS_OK;

/// `lxb_dom_node_insert_child` and its two siblings share this shape, which is
/// what lets [`splice_or_insert`] hold the fragment rule in one place.
type InsertFn = unsafe extern "C" fn(*mut LxbNode, *mut LxbNode);

pub use crate::bridge::string::mkr_ruby_verified_data;
pub use crate::dom_adapter::dom_index::mkr_parsed_dom_index_invalidate;
pub use crate::dom_adapter::text_index::mkr_parsed_text_index_invalidate;
pub use crate::glue::fragment::mkr_emit_append;
pub use crate::glue::fragment::mkr_emit_before;
pub use crate::glue::fragment::mkr_html_import_deep;
pub use crate::glue::fragment::mkr_import_fragment_children;
pub use crate::glue::fragment::mkr_run_fragment_parser;

/* ------------------------------------------------------------------ *
 * shared helpers                                                     *
 * ------------------------------------------------------------------ */

fn err(msg: &str) -> Error {
    Error::new(unsafe { error_class() }, msg.to_owned())
}

/// Drop the DOM and text indexes so the next query rebuilds them.
unsafe fn invalidate(document: Value) {
    let p = crate::glue::doc::doc_parsed_known(document.as_raw());
    mkr_parsed_dom_index_invalidate(p);
    mkr_parsed_text_index_invalidate(p);
}

/// Every mutator unwraps `self` through here: a node the caller has frozen is
/// immutable, so raise FrozenError rather than silently editing it. The readers
/// use [`unwrap`] directly.
unsafe fn unwrap_mutable(this: super::HtmlSelf) -> *mut LxbNode {
    let rb_self = this.value;
    rb_sys::rb_check_frozen(rb_self.as_raw());
    this.node
}

/// An HTML node argument. Routes through the HTML unwrap so an XML node is
/// rejected before its arena pointer reaches Lexbor.
unsafe fn arg_node(v: Value) -> Result<*mut LxbNode, Error> {
    unwrap(v)
}

/// Copy `node` into `doc`, for a node that came from another document - this
/// half of the DOM's adopt. **Raises** rather than returning a partial node.
unsafe fn adopt_copy(doc: *mut LxbDoc, node: *mut LxbNode) -> *mut LxbNode {
    mkr_html_import_deep(doc, node)
}

/// The other half: take `node` out of the document it came from, so the whole
/// thing reads as the move the DOM says appendChild performs.
unsafe fn adopt_release(node: *mut LxbNode) {
    if (*node).type_ == ty::FRAGMENT {
        /* A fragment contributes its children; the DOM leaves a spliced one
         * empty, so empty the source rather than detaching it. */
        loop {
            let c = (*node).first_child;
            if c.is_null() {
                break;
            }
            lxb::lxb_dom_node_remove(c);
        }
    } else if !(*node).parent.is_null() {
        lxb::lxb_dom_node_remove(node);
    }
}

/// Validate that `rb_incoming` may be placed relative to `reference`, detach it
/// from any current parent (move semantics), and return the node to actually
/// insert.
///
/// For a node from another document that is its COPY, so the returned node is
/// not always the one passed in and the caller must insert - and hand back -
/// what this returns. The second element is the argument in that case, for
/// [`inserted_result`] to release once the insert has gone through; holding the
/// Ruby `Value` rather than the raw node also keeps the source document
/// reachable until then.
unsafe fn prepare_insert(
    reference: *mut LxbNode,
    rb_incoming: Value,
) -> Result<(*mut LxbNode, Option<Value>), Error> {
    let incoming = arg_node(rb_incoming)?;

    if (*incoming).type_ == ty::ATTRIBUTE {
        return Err(err("an attribute node cannot be inserted into the tree"));
    }
    /* `incoming` must not be an inclusive ancestor of `reference`. */
    let mut p = reference;
    while !p.is_null() {
        if p == incoming {
            return Err(err("cannot insert a node into its own subtree"));
        }
        p = (*p).parent;
    }
    if (*reference).owner_document != (*incoming).owner_document {
        return Ok((
            adopt_copy((*reference).owner_document, incoming),
            Some(rb_incoming),
        ));
    }
    if !(*incoming).parent.is_null() {
        lxb::lxb_dom_node_remove(incoming);
    }
    Ok((incoming, None))
}

/// The value an insertion verb hands back: its argument, or - when the node was
/// adopted - the node now in the tree, which is a different object. Finishing
/// the adoption here keeps the release after the insert, where it belongs.
unsafe fn inserted_result(
    rb_self: Value,
    rb_arg: Value,
    inserted: *mut LxbNode,
    adopt_from: Option<Value>,
) -> Result<Value, Error> {
    match adopt_from {
        None => Ok(rb_arg),
        Some(src) => {
            adopt_release(arg_node(src)?);
            Ok(wrap(inserted, node_document(rb_self)?))
        }
    }
}

/// Whether inserting `n` contributes an element at the insertion point: `n` is
/// an element, or a fragment (spliced as its children) that carries one.
///
/// A fragment bypasses the per-node guard because [`splice_or_insert`] hands its
/// children straight to Lexbor, so the element-vs-doctype order is checked here
/// instead.
unsafe fn contributes_element(n: *const LxbNode) -> bool {
    if (*n).type_ == ty::ELEMENT {
        return true;
    }
    if (*n).type_ == ty::FRAGMENT {
        let mut c = (*n).first_child;
        while !c.is_null() {
            if (*c).type_ == ty::ELEMENT {
                return true;
            }
            c = (*c).next;
        }
    }
    false
}

/// WHATWG doctype ordering at the document node, fail-closed so a `<!DOCTYPE>`
/// always precedes the document element:
///
/// - a DocumentType may only be a document child, at most one, with no element
///   before it; and, symmetrically,
/// - an element may not be inserted ahead of an existing doctype.
///
/// `parent` is the future parent, `before` the child `incoming` is inserted
/// before (NULL = append at the end), `exclude` a node the same operation
/// removes (the replace target) or NULL. Called BEFORE any link change - that
/// is, before the detach in [`prepare_insert`].
unsafe fn guard_doc_child_order(
    parent: *const LxbNode,
    before: *const LxbNode,
    exclude: *const LxbNode,
    incoming: *const LxbNode,
) -> Result<(), Error> {
    if (*incoming).type_ == ty::DOCTYPE {
        if parent.is_null() || (*parent).type_ != ty::DOCUMENT {
            return Err(err("a doctype node can only be a child of the document"));
        }
        let mut c = (*parent).first_child as *const LxbNode;
        while !c.is_null() {
            if c != exclude && c != incoming && (*c).type_ == ty::DOCTYPE {
                return Err(err("the document already has a doctype"));
            }
            c = (*c).next;
        }
        /* No element before the doctype. `before == NULL` (append) means every
         * element child precedes the tail; otherwise only the siblings ahead of
         * `before` are in the way. */
        let mut c = (*parent).first_child as *const LxbNode;
        while c != before {
            if c != exclude && c != incoming && (*c).type_ == ty::ELEMENT {
                return Err(err("a doctype must precede the document element"));
            }
            c = (*c).next;
        }
        return Ok(());
    }

    if contributes_element(incoming) && !parent.is_null() && (*parent).type_ == ty::DOCUMENT {
        /* Symmetric: the element (or a fragment carrying one) would sit at
         * `before`'s slot, so a doctype at or after `before` would end up behind
         * it - reject. */
        let mut c = before;
        while !c.is_null() {
            if c != exclude && c != incoming && (*c).type_ == ty::DOCTYPE {
                return Err(err("a doctype must precede the document element"));
            }
            c = (*c).next;
        }
    }
    Ok(())
}

/// Insert `node` relative to `anchor`, or - when `node` is a document fragment -
/// splice its children there in order, leaving the fragment empty.
///
/// With `advance` (insert_after semantics) each spliced child becomes the anchor
/// for the next, so document order is preserved; child/before splices keep a
/// fixed anchor. The one place the fragment-vs-single-node rule lives.
unsafe fn splice_or_insert(
    mut anchor: *mut LxbNode,
    node: *mut LxbNode,
    insert: InsertFn,
    advance: bool,
) {
    if (*node).type_ != ty::FRAGMENT {
        insert(anchor, node);
        return;
    }
    loop {
        let c = (*node).first_child;
        if c.is_null() {
            break;
        }
        lxb::lxb_dom_node_remove(c);
        insert(anchor, c);
        if advance {
            anchor = c; /* keep document order after the reference node */
        }
    }
}

/* ------------------------------------------------------------------ *
 * tree mutation                                                      *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child. Appends as the last child; a document
/// fragment contributes its children rather than itself.
pub fn add_child(_ruby: &Ruby, this: super::HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let parent = unwrap_mutable(this);
        guard_doc_child_order(
            parent,
            core::ptr::null(),
            core::ptr::null(),
            arg_node(rb_child)?,
        )?;
        let (ins, adopt_from) = prepare_insert(parent, rb_child)?;
        splice_or_insert(parent, ins, lxb::lxb_dom_node_insert_child, false);
        invalidate(this.document);
        inserted_result(rb_self, rb_child, ins, adopt_from)
    }
}

/// `node << child` -> node (chainable).
pub fn lshift(ruby: &Ruby, this: super::HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    add_child(ruby, this, rb_child)?;
    Ok(rb_self)
}

pub fn before(_ruby: &Ruby, this: super::HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(this);
        if (*reference).parent.is_null() {
            return Err(err("cannot add a sibling to a node with no parent"));
        }
        guard_doc_child_order(
            (*reference).parent,
            reference,
            core::ptr::null(),
            arg_node(rb_node)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
        splice_or_insert(reference, ins, lxb::lxb_dom_node_insert_before, false);
        invalidate(this.document);
        inserted_result(rb_self, rb_node, ins, adopt_from)
    }
}

pub fn after(_ruby: &Ruby, this: super::HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(this);
        if (*reference).parent.is_null() {
            return Err(err("cannot add a sibling to a node with no parent"));
        }
        guard_doc_child_order(
            (*reference).parent,
            (*reference).next,
            core::ptr::null(),
            arg_node(rb_node)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
        splice_or_insert(reference, ins, lxb::lxb_dom_node_insert_after, true);
        invalidate(this.document);
        inserted_result(rb_self, rb_node, ins, adopt_from)
    }
}

/// `node.remove` / `node.unlink` -> node. Detaches from the tree; the node stays
/// usable, because the arena owns it.
pub fn remove(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ == ty::ATTRIBUTE {
            return Err(err("use delete(name) to remove an attribute"));
        }
        if !(*node).parent.is_null() {
            lxb::lxb_dom_node_remove(node);
            invalidate(this.document);
        }
        Ok(rb_self)
    }
}

/// `node.replace(other)` -> other. Puts `other` where `node` is, detaches node.
pub fn replace(_ruby: &Ruby, this: super::HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(this);
        if (*reference).parent.is_null() {
            return Err(err("cannot replace a node with no parent"));
        }
        guard_doc_child_order(
            (*reference).parent,
            reference,
            reference,
            arg_node(rb_other)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_other)?;
        splice_or_insert(reference, ins, lxb::lxb_dom_node_insert_before, false);
        lxb::lxb_dom_node_remove(reference);
        invalidate(this.document);
        inserted_result(rb_self, rb_other, ins, adopt_from)
    }
}

/* ------------------------------------------------------------------ *
 * attribute mutation                                                 *
 * ------------------------------------------------------------------ */

/// `element[name] = value` -> value.
pub fn aset(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Err(err("cannot set an attribute on a non-element node"));
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr())?;
        let vv = mkr_ruby_verified_data(rb_value.as_raw(), c"attribute value".as_ptr())?;
        let attr = lxb::lxb_dom_element_set_attribute(
            node as *mut LxbElement,
            nv.as_ptr() as *const u8,
            nv.len(),
            vv.as_ptr() as *const u8,
            vv.len(),
        );
        if attr.is_null() {
            return Err(err("failed to set attribute"));
        }
        invalidate(this.document);
        Ok(rb_value)
    }
}

/// An attribute's OWN namespace id: the one recorded by `set_attribute_ns`
/// (which differs from the owner element's), else the null namespace - a
/// normally-set or parsed attribute inherits the element's ns, which for
/// matching purposes is null (an unprefixed attribute is namespaceless).
unsafe fn attr_own_ns(at: *const LxbAttr) -> usize {
    let owner = (*at).owner;
    if !owner.is_null() && (*at).node.ns != (*owner).node.ns {
        return (*at).node.ns;
    }
    NS_UNDEF
}

/// Find the attribute on `el` matching (ns_id, local_name) case-sensitively.
///
/// The DOM keys attributes on (namespace, local name), so two with the same
/// qualified name in different namespaces coexist - which Lexbor's
/// by-qualified-name, case-insensitive-for-HTML lookup cannot express.
unsafe fn attr_find_ns(el: *mut LxbElement, ns_id: usize, local: &[u8]) -> *mut LxbAttr {
    let mut at = (*el).first_attr;
    while !at.is_null() {
        if attr_own_ns(at) == ns_id {
            /* Compare the case-PRESERVED local name (the suffix of the
             * qualified name): Lexbor lower-cases the stored local_name even
             * when the qualified name keeps its case, but setAttributeNS is
             * case-sensitive. */
            let mut qlen = 0usize;
            let mut llen = 0usize;
            let q = lxb::lxb_dom_attr_qualified_name(at, &mut qlen);
            lxb::lxb_dom_attr_local_name(at, &mut llen);
            if !q.is_null()
                && qlen >= llen
                && core::slice::from_raw_parts(q.add(qlen - llen), llen) == local
            {
                return at;
            }
        }
        at = (*at).next;
    }
    core::ptr::null_mut()
}

/// Intern `uri` in the document's namespace table, or the null namespace for an
/// empty one.
unsafe fn intern_ns(node: *mut LxbNode, uri: &[u8]) -> usize {
    if uri.is_empty() {
        return NS_UNDEF;
    }
    let doc = (*node).owner_document;
    if doc.is_null() || (*doc).ns.is_null() {
        return NS_UNDEF;
    }
    let d = lxb::lxb_ns_append((*doc).ns as *mut c_void, uri.as_ptr(), uri.len());
    if d.is_null() {
        NS_UNDEF
    } else {
        (*d).ns_id
    }
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
///
/// Stores the attribute under its qualified name (case-preserved -
/// setAttributeNS is case-sensitive, unlike the HTML setAttribute family) and
/// records its OWN namespace on the attr node, so `namespaceURI` and
/// getAttributeNS resolve it. nil or `""` stores the null namespace.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Err(err("cannot set an attribute on a non-element node"));
        }
        let el = node as *mut LxbElement;

        let qv = mkr_ruby_verified_text(rb_qname.as_raw(), c"attribute qualified name".as_ptr())?;
        let vv = mkr_ruby_verified_data(rb_value.as_raw(), c"attribute value".as_ptr())?;

        let nv = if rb_ns.is_nil() {
            None
        } else {
            Some(mkr_ruby_verified_text(
                rb_ns.as_raw(),
                c"namespace".as_ptr(),
            )?)
        };
        let ns_bytes: &[u8] = match &nv {
            Some(nv) => nv.bytes(),
            None => &[],
        };
        let have_ns = !ns_bytes.is_empty();

        /* Intern the wanted namespace so the existing attribute is matched on
         * (namespace, local name) - the DOM key - rather than on the qualified
         * name. */
        let want_ns = intern_ns(node, ns_bytes);

        let qname = core::slice::from_raw_parts(qv.as_ptr() as *const u8, qv.len());
        let local = match qname.iter().position(|&b| b == b':') {
            Some(i) => &qname[i + 1..],
            None => qname,
        };

        /* A match keeps its qualified name (so re-setting with a different
         * prefix leaves the prefix unchanged); only the value updates. A miss
         * appends a new attribute, even when its qualified name collides with an
         * existing one in a different namespace. */
        let existing = attr_find_ns(el, want_ns, local);
        let outcome = if !existing.is_null() {
            if lxb::lxb_dom_attr_set_value(existing, vv.as_ptr() as *const u8, vv.len())
                != STATUS_OK
            {
                Err(err("failed to set attribute value"))
            } else {
                Ok(())
            }
        } else {
            let attr = lxb::lxb_dom_attr_interface_create((*node).owner_document);
            if attr.is_null() {
                Err(err("failed to create attribute"))
            } else {
                /* A fresh attr is calloc'd, so node.ns is already the null
                 * namespace; only the namespaced setter changes it. */
                let st = if have_ns {
                    lxb::lxb_dom_attr_set_name_ns(
                        attr,
                        ns_bytes.as_ptr(),
                        ns_bytes.len(),
                        qv.as_ptr() as *const u8,
                        qv.len(),
                        false,
                    )
                } else {
                    lxb::lxb_dom_attr_set_name(attr, qv.as_ptr() as *const u8, qv.len(), false)
                };
                if st != STATUS_OK
                    || lxb::lxb_dom_attr_set_value(attr, vv.as_ptr() as *const u8, vv.len())
                        != STATUS_OK
                {
                    /* Leave the un-appended attr for the document arena to free
                     * wholesale (this module's "never destroy" convention). */
                    Err(err("failed to set namespaced attribute"))
                } else {
                    lxb::lxb_dom_element_attr_append(el, attr);
                    Ok(())
                }
            }
        };

        outcome?;

        invalidate(this.document);
        Ok(rb_value)
    }
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
///
/// Removes the attribute matching (namespace, local name) - the DOM key - so a
/// namespaced attribute goes without disturbing a same-qualified-name one in
/// another namespace, which removal by qualified name would.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let el = node as *mut LxbElement;

        let lv = mkr_ruby_verified_text(rb_local.as_raw(), c"attribute local name".as_ptr())?;

        let mut want_ns = NS_UNDEF;
        if !rb_ns.is_nil() {
            let nv = mkr_ruby_verified_text(rb_ns.as_raw(), c"namespace".as_ptr())?;
            if nv.len() != 0 {
                want_ns = intern_ns(node, nv.bytes());
            }
        }

        let local = lv.bytes();
        let attr = attr_find_ns(el, want_ns, local);

        if !attr.is_null() {
            lxb::lxb_dom_element_attr_remove(el, attr);
            invalidate(this.document);
        }
        Ok(ruby.qnil().as_value())
    }
}

/// `element.name = new_name` -> new_name.
///
/// Renames in place with identity preserved: create a throwaway element with the
/// new name so the document interns it, copy its name fields onto this node,
/// then discard it.
pub fn set_name(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Err(err("name= is only supported on elements"));
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"element name".as_ptr())?;
        let fresh = lxb::lxb_dom_document_create_element(
            (*node).owner_document,
            nv.as_ptr() as *const u8,
            nv.len(),
            core::ptr::null_mut(),
        );
        if fresh.is_null() {
            return Err(err("failed to rename element"));
        }

        let el = node as *mut LxbElement;
        (*el).node.local_name = (*fresh).node.local_name;
        (*el).node.prefix = (*fresh).node.prefix;
        (*el).node.ns = (*fresh).node.ns;
        (*el).upper_name = (*fresh).upper_name;
        (*el).qualified_name = (*fresh).qualified_name;

        lxb::lxb_dom_node_destroy(fresh as *mut LxbNode);
        /* The element's tag id (local_name) is the key the element-by-tag index
         * buckets on and the //tag fast path serves from; renaming changes it,
         * so a persisted index would miss the element under its new name - a
         * truncated, wrong //newtag result. Drop the indexes like every other
         * mutator. */
        invalidate(this.document);
        Ok(rb_name)
    }
}

/// `node.content = text` -> text. The DOM textContent setter: for an element
/// this replaces all children with a single text node; for a character-data node
/// it sets the data.
pub fn set_content(_ruby: &Ruby, this: super::HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        let tv = mkr_ruby_verified_data(rb_text.as_raw(), c"node content".as_ptr())?;
        let st = lxb::lxb_dom_node_text_content_set(node, tv.as_ptr() as *const u8, tv.len());
        if st != STATUS_OK {
            return Err(err("failed to set node content"));
        }
        invalidate(this.document);
        Ok(rb_text)
    }
}

/// `element.delete(name)` -> self. Removes the attribute if present.
pub fn delete(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Ok(rb_self);
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr())?;
        lxb::lxb_dom_element_remove_attribute(
            node as *mut LxbElement,
            nv.as_ptr() as *const u8,
            nv.len(),
        );
        invalidate(this.document);
        Ok(rb_self)
    }
}

/* ------------------------------------------------------------------ *
 * inner_html= / outer_html=                                          *
 * ------------------------------------------------------------------ */

/// Parse callback for `mkr_run_fragment_parser`: Lexbor's element-context
/// fragment parser, which is what `inner_html=`/`outer_html=` need. `ctx` is the
/// context element.
unsafe extern "C" fn parse_fragment_by_context(
    parser: *mut c_void,
    src: *const u8,
    len: usize,
    ctx: *mut c_void,
) -> *mut LxbNode {
    /* The generated declaration is typed to Lexbor's parser and element
     * interfaces; the callback contract `mkr_run_fragment_parser` passes is
     * representation-opaque, so the casts happen here rather than in a second
     * declaration of the same symbol. */
    lxb::lxb_html_parse_fragment(parser as *mut _, ctx as *mut _, src, len)
}

/// Parse `rb_html` as a fragment in the context of `context_el` and splice the
/// imported nodes via `emit`.
///
/// UTF-8 decoding (browser-compatible: invalid bytes become U+FFFD) and the
/// import + `<template>`-content fixup are shared with the DocumentFragment
/// paths in `glue::fragment`.
unsafe fn parse_fragment_into(
    context_el: *mut LxbNode,
    rb_html: Value,
    doc: *mut LxbDoc,
    emit: unsafe extern "C" fn(*mut LxbNode, *mut c_void),
    u: *mut c_void,
) -> Result<(), Error> {
    let html = Value::from_raw(rb_sys::rb_String(rb_html.as_raw()));
    let frag = mkr_run_fragment_parser(
        html.as_raw(),
        parse_fragment_by_context,
        context_el as *mut c_void,
    );

    let imported = mkr_import_fragment_children(doc, frag, emit, u);

    /* lxb_html_parse_fragment built the fragment in a TRANSIENT document that
     * destroying the parser does NOT free (measured: one document leaked per
     * inner_html=/outer_html= call). Our imported copies live in `doc`, so the
     * transient document is destroyed here - frag->owner_document is valid after
     * the parser is gone. */
    lxb::lxb_html_document_destroy((*frag).owner_document as *mut _);
    let _anchor = html;

    /* AFTER the destroy: returning the error before it would skip the free
     * above, leaking one Lexbor document per failure - the very leak that free
     * exists to fix. */
    if imported != 0 {
        return Err(err("failed to import a fragment child"));
    }
    Ok(())
}

/// `element.inner_html = html` -> html. Replaces the element's children.
pub fn set_inner_html(_ruby: &Ruby, this: super::HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        if (*node).type_ != ty::ELEMENT {
            return Err(err("inner_html= requires an element"));
        }

        /* Detach the existing children; the arena reclaims them at document
         * destroy. */
        loop {
            let c = (*node).first_child;
            if c.is_null() {
                break;
            }
            lxb::lxb_dom_node_remove(c);
        }

        parse_fragment_into(
            node,
            rb_html,
            (*node).owner_document,
            mkr_emit_append,
            node as *mut c_void,
        )?;
        invalidate(this.document);
        Ok(rb_html)
    }
}

/// `node.outer_html = html` -> html. Replaces the node itself with the parse.
pub fn set_outer_html(_ruby: &Ruby, this: super::HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(this);
        let parent = (*node).parent;
        if parent.is_null() || (*parent).type_ != ty::ELEMENT {
            return Err(err("outer_html= requires a node with a parent element"));
        }

        /* Parse in the parent's context, splice the imported nodes before self. */
        parse_fragment_into(
            parent,
            rb_html,
            (*node).owner_document,
            mkr_emit_before,
            node as *mut c_void,
        )?;
        lxb::lxb_dom_node_remove(node);
        invalidate(this.document);
        Ok(rb_html)
    }
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

pub fn create_element(_ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"element name".as_ptr())?;
        let el = lxb::lxb_dom_document_create_element(
            doc,
            nv.as_ptr() as *const u8,
            nv.len(),
            core::ptr::null_mut(),
        );
        if el.is_null() {
            return Err(err("failed to create element"));
        }
        Ok(wrap(el as *mut LxbNode, rb_self))
    }
}

pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let tv = mkr_ruby_verified_data(rb_text.as_raw(), c"text content".as_ptr())?;
        let t = lxb::lxb_dom_document_create_text_node(doc, tv.as_ptr() as *const u8, tv.len());
        if t.is_null() {
            return Err(err("failed to create text node"));
        }
        Ok(wrap(t as *mut LxbNode, rb_self))
    }
}

pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let tv = mkr_ruby_verified_data(rb_text.as_raw(), c"comment content".as_ptr())?;
        let c = lxb::lxb_dom_document_create_comment(doc, tv.as_ptr() as *const u8, tv.len());
        if c.is_null() {
            return Err(err("failed to create comment"));
        }
        Ok(wrap(c as *mut LxbNode, rb_self))
    }
}

/// `Document#create_processing_instruction(target, data)` - the DOM
/// createProcessingInstruction: a detached PI owned by this document. Lexbor
/// validates the target, so an invalid one fails closed.
pub fn create_pi(
    _ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let tv = mkr_ruby_verified_text(
            rb_target.as_raw(),
            c"processing instruction target".as_ptr(),
        )?;
        let dv = mkr_ruby_verified_text(rb_data.as_raw(), c"processing instruction data".as_ptr())?;
        let pi = lxb::lxb_dom_document_create_processing_instruction(
            doc,
            tv.as_ptr() as *const u8,
            tv.len(),
            dv.as_ptr() as *const u8,
            dv.len(),
        );
        if pi.is_null() {
            return Err(err("failed to create processing instruction"));
        }
        Ok(wrap(pi as *mut LxbNode, rb_self))
    }
}

/// `Document#create_document_type(name, public_id = "", system_id = "")` - the
/// DOM DOMImplementation.createDocumentType: a detached DocumentType owned by
/// this document, to be placed before the document element (the tree guards
/// enforce that). An empty or omitted public/system id is treated as absent.
/// Lexbor validates the name as a DOM Name, so an invalid one fails closed.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let args =
        magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
            args,
        )?;
    let (rb_name,) = args.required;
    let (rb_pub, rb_sys_) = args.optional;

    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"doctype name".as_ptr())?;
        if !lxb::lxb_dom_document_type_valid_name(nv.as_ptr() as *const u8, nv.len()) {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "invalid doctype name",
            ));
        }

        let zero = crate::glue::abi::RubyText::absent;
        let pv = match rb_pub.filter(|v| !v.is_nil()) {
            Some(v) => mkr_ruby_verified_text(v.as_raw(), c"doctype public id".as_ptr())?,
            None => zero(),
        };
        let sv = match rb_sys_.filter(|v| !v.is_nil()) {
            Some(v) => mkr_ruby_verified_text(v.as_raw(), c"doctype system id".as_ptr())?,
            None => zero(),
        };
        let pub_ptr = if pv.len() != 0 {
            pv.as_ptr() as *const u8
        } else {
            core::ptr::null()
        };
        let sys_ptr = if sv.len() != 0 {
            sv.as_ptr() as *const u8
        } else {
            core::ptr::null()
        };

        /* The exception code is generated as a plain int; it is written but not
         * read - a NULL dt is the failure signal, as in the C. */
        let mut code: core::ffi::c_int = 0;
        let dt = lxb::lxb_dom_document_type_create(
            doc,
            nv.as_ptr() as *const u8,
            nv.len(),
            pub_ptr,
            pv.len(),
            sys_ptr,
            sv.len(),
            &mut code,
        );
        if dt.is_null() {
            return Err(err("failed to create doctype"));
        }
        /* create() interned the name ASCII-lowercased (the attr local-name
         * hash); DOM createDocumentType preserves case, so re-intern it raw and
         * repoint. */
        let nd = lxb::lxb_dom_attr_qualified_name_append(
            (*doc).attrs as *mut c_void,
            nv.as_ptr() as *const u8,
            nv.len(),
        );
        if nd.is_null() {
            return Err(err("failed to intern doctype name"));
        }
        (*dt).name = (*nd).attr_id;

        /* create() leaves an absent public/system id as a {NULL,0} lexbor_str,
         * but lxb_dom_document_type_interface_clone (used by
         * import_node/clone_node) runs lexbor_str_copy, which fails on a NULL
         * source - so an absent-id doctype would be unimportable. Initialise
         * them to an allocated empty string: the accessor and serializer both
         * key on length == 0, so this reads as nil/absent to callers but is
         * non-NULL to the cloner. */
        if (*dt).public_id.data.is_null() {
            lxb::lexbor_str_init(&mut (*dt).public_id, (*doc).text, 0);
        }
        if (*dt).system_id.data.is_null() {
            lxb::lexbor_str_init(&mut (*dt).system_id, (*doc).text, 0);
        }
        Ok(wrap(dt as *mut LxbNode, rb_self))
    }
}

/// `Document#create_document_fragment` - the DOM createDocumentFragment: an
/// EMPTY DocumentFragment owned by this document, unlike `#fragment` /
/// `DocumentFragment.parse`, which parse HTML.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    unsafe {
        let doc = mkr_html_doc_unwrap(rb_self.as_raw())?;
        let f = lxb::lxb_dom_document_create_document_fragment(doc);
        if f.is_null() {
            return Err(err("failed to create document fragment"));
        }
        Ok(wrap(f as *mut LxbNode, rb_self))
    }
}
