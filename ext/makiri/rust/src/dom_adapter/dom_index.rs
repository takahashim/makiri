//! The per-document DOM indices (dom_adapter/dom_index.c).
//!
//! Two indices, built in one object because they share a walk:
//!
//! - **attribute -> owner element**, an open-addressing hash keyed on the
//!   `lxb_dom_attr_t` pointer. Lexbor sets neither `attr->owner` nor
//!   `attr->node.parent`, so without this an attribute has no way back to its
//!   element.
//! - **tag id -> elements**, a CSR layout: a flat `tag_nodes` array of every
//!   element grouped by tag id and kept in document order, with `tag_off`
//!   prefix-sum offsets, so bucket `t` is `tag_nodes[tag_off[t]..tag_off[t+1]]`.
//!   This is what lets the XPath engine answer `//tag` without walking.
//!
//! Two tree passes: count and size, then fill. Each table is therefore sized
//! once and never rehashes mid-build, which is what keeps the OOM path trivial -
//! there is nothing half-built to unwind.
//!
//! # The build also writes to the tree
//!
//! Filling backfills each attribute node's `parent` to its owner element. Lexbor
//! leaves it NULL; setting it to the semantically correct owner is safe because
//! Lexbor walks the tree through `first_child`/`next` and an attribute never
//! appears in that chain, and it lets the XPath engine read `node.parent` for the
//! parent and ancestor axes and for document order without special-casing
//! attributes.
//!
//! # Fail-closed
//!
//! An allocation failure leaves the index UNBUILT and returns "no index", so the
//! caller either walks or raises. It must not leave a partially filled one: a
//! lookup that misses reads as "this attribute has no owner", which is a
//! well-formed wrong answer.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use crate::falloc::{try_box, try_vec_with_capacity};
use crate::lexbor_abi::{self as lxb, preorder_next, LxbAttr, LxbDoc, LxbElement, LxbNode};
use crate::xpath::runtime_abi::cache::ptr_hash;

type Parsed = lxb::parsed::Parsed;

const NODE_TYPE_ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
const TAG_UNDEF: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize;

/// Tag buckets cover only Lexbor's STATIC tag-id range `[1, LXB_TAG__LAST_ENTRY)`.
///
/// A custom element's tag id is the pointer to its interned tag data
/// (`lxb_tag_append` sets `data->tag_id = (lxb_tag_id_t) data`), an enormous
/// value that cannot key a dense array. Those elements are simply left out, and
/// `//customtag` falls back to a tree walk - rare in practice.
const TAG_INDEX_CAP: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__LAST_ENTRY as usize;

/// One attr->owner slot. A null `attr` marks an empty slot; there are no
/// deletions, so linear probing never needs a tombstone.
#[derive(Clone, Copy)]
struct AttrSlot {
    attr: *mut LxbAttr,
    owner: *mut LxbNode,
}

const EMPTY_SLOT: AttrSlot = AttrSlot {
    attr: core::ptr::null_mut(),
    owner: core::ptr::null_mut(),
};

struct DomIndex {
    slots: Vec<AttrSlot>,
    /// A power of two, or 0 when the document has no attributes.
    cap: usize,

    /// Every indexed element, grouped by tag id, in document order.
    tag_nodes: Vec<*mut LxbNode>,
    /// `tag_max + 2` entries, or empty.
    tag_off: Vec<usize>,
    /// The highest tag id present.
    tag_max: usize,
    /// Any element with a namespace other than HTML.
    has_foreign: bool,
}

impl DomIndex {
    #[inline]
    fn attr_slot(&self, attr: *const LxbAttr) -> usize {
        (ptr_hash(attr) as usize) & (self.cap - 1)
    }

    /// The table is sized at load factor <= 0.5 and never grows during the fill,
    /// so a free slot always exists and this probe terminates.
    fn attr_insert(&mut self, attr: *mut LxbAttr, owner: *mut LxbNode) {
        let mut i = self.attr_slot(attr);
        while !self.slots[i].attr.is_null() {
            if self.slots[i].attr == attr {
                return; /* already mapped; an attribute has one owner */
            }
            i = (i + 1) & (self.cap - 1);
        }
        self.slots[i] = AttrSlot { attr, owner };
    }

    fn attr_owner(&self, attr: *mut LxbAttr) -> *mut LxbNode {
        if self.cap == 0 {
            return core::ptr::null_mut();
        }
        let mut i = self.attr_slot(attr);
        loop {
            if self.slots[i].attr.is_null() {
                return core::ptr::null_mut();
            }
            if self.slots[i].attr == attr {
                return self.slots[i].owner;
            }
            i = (i + 1) & (self.cap - 1);
        }
    }
}

/// Walk an element's own attribute list.
#[inline]
unsafe fn each_attr(node: *mut LxbNode, mut f: impl FnMut(*mut LxbAttr)) {
    let mut a = lxb::lxb_dom_element_first_attribute_noi(node as *mut LxbElement);
    while !a.is_null() {
        f(a);
        a = lxb::lxb_dom_element_next_attribute_noi(a);
    }
}

/// An element's tag id, when it is one this index buckets.
#[inline]
unsafe fn indexable_tag(node: *const LxbNode) -> Option<usize> {
    let tag = (*node).local_name;
    if tag != TAG_UNDEF && tag < TAG_INDEX_CAP {
        Some(tag)
    } else {
        None
    }
}

/// Build over `doc`. `None` on allocation failure, with nothing written to the
/// tree yet - the backfill happens only in the fill pass, which cannot fail.
unsafe fn build(doc: *mut LxbDoc) -> Option<DomIndex> {
    let root = doc as *mut LxbNode;

    /* Pass 1: one walk to size everything. */
    let mut counts = [0usize; TAG_INDEX_CAP];
    let mut n_attrs = 0usize;
    let mut n_indexed = 0usize;
    let mut tag_max = 0usize;
    let mut has_foreign = false;

    let mut node: *mut LxbNode = root;
    while !node.is_null() {
        if (*node).type_ == NODE_TYPE_ELEMENT {
            if (*node).ns != NS_HTML {
                has_foreign = true;
            }
            if let Some(tag) = indexable_tag(node) {
                counts[tag] += 1;
                n_indexed += 1;
                if tag > tag_max {
                    tag_max = tag;
                }
            }
            each_attr(node, |_| n_attrs += 1);
        }
        node = preorder_next(node, root);
    }

    let mut idx = DomIndex {
        slots: Vec::new(),
        cap: 0,
        tag_nodes: Vec::new(),
        tag_off: Vec::new(),
        tag_max,
        has_foreign,
    };

    /* The attr->owner table, at load factor <= 0.5. */
    if n_attrs > 0 {
        let cap = n_attrs.checked_mul(2)?.checked_next_power_of_two()?.max(8);
        idx.slots = try_vec_with_capacity(cap)?;
        idx.slots.resize(cap, EMPTY_SLOT); /* reserved above; cannot allocate */
        idx.cap = cap;
    }

    /* The tag CSR. `cursor` is scratch: a copy of the offsets, advanced as
     * elements are scattered, which is what preserves document order within a
     * bucket. */
    let mut cursor: Vec<usize> = Vec::new();
    if n_indexed > 0 {
        let noff = tag_max.checked_add(2)?;
        idx.tag_off = try_vec_with_capacity(noff)?;
        idx.tag_nodes = try_vec_with_capacity(n_indexed)?;
        cursor = try_vec_with_capacity(noff)?;

        /* Prefix-sum the per-tag counts into offsets. The running sum is
         * checked, not wrapped: a wrapped offset would make a bucket read the
         * wrong slice of tag_nodes. */
        idx.tag_off.push(0);
        let mut running = 0usize;
        for c in counts.iter().take(tag_max + 1) {
            running = running.checked_add(*c)?;
            idx.tag_off.push(running);
        }
        cursor.extend_from_slice(&idx.tag_off); /* reserved above */
        idx.tag_nodes.resize(n_indexed, core::ptr::null_mut());
    }

    /* Pass 2: fill. No failure path - every array is already the right size,
     * which is what lets the backfill below happen without a half-built index
     * ever being observable. */
    node = root;
    while !node.is_null() {
        if (*node).type_ == NODE_TYPE_ELEMENT {
            if let Some(tag) = indexable_tag(node) {
                idx.tag_nodes[cursor[tag]] = node;
                cursor[tag] += 1;
            }
            each_attr(node, |a| {
                idx.attr_insert(a, node);
                /* Backfill the attribute's parent - see the module docs. */
                (*a).node.parent = node;
            });
        }
        node = preorder_next(node, root);
    }

    Some(idx)
}

/// The built index for `p`, building it if needed. NULL on allocation failure,
/// which leaves the cache empty so a later call retries.
unsafe fn ensure(p: *mut Parsed) -> *mut DomIndex {
    if p.is_null() || (*p).doc.is_null() {
        return core::ptr::null_mut();
    }
    if !(*p).dom_index.is_null() {
        return (*p).dom_index as *mut DomIndex;
    }
    let built = match build((*p).doc as *mut LxbDoc) {
        Some(i) => i,
        None => return core::ptr::null_mut(),
    };
    let boxed = match try_box(built) {
        Ok(b) => b,
        Err(_) => return core::ptr::null_mut(),
    };
    let raw = Box::into_raw(boxed);
    (*p).dom_index = raw as *mut c_void;
    raw
}

/* ------------------------------------------------------------------ *
 * public compat API                                                  *
 * ------------------------------------------------------------------ */

/// The element that owns `attr`, or NULL - both for "not in this document" and
/// for "the index could not be built". A caller that must tell those apart calls
/// [`parsed_dom_index_build`] first; `Attribute#parent` does exactly that,
/// because a nil parent there would be a navigation answer, not an error.
pub unsafe fn parsed_attr_owner(p: *mut Parsed, attr: *mut LxbAttr) -> *mut LxbNode {
    if attr.is_null() {
        return core::ptr::null_mut();
    }
    let idx = ensure(p);
    if idx.is_null() {
        return core::ptr::null_mut();
    }
    (*idx).attr_owner(attr)
}

/// Build the index now (idempotent). `true` on success, `false` on allocation
/// failure.
pub unsafe fn parsed_dom_index_build(p: *mut Parsed) -> bool {
    !ensure(p).is_null()
}

/// Drop the index so the next query rebuilds it. Called from the one mutation
/// hook, beside the text index's.
pub unsafe fn parsed_dom_index_invalidate(p: *mut Parsed) {
    if p.is_null() {
        return;
    }
    dom_index_free((*p).dom_index);
    (*p).dom_index = core::ptr::null_mut();
}

/// NULL-safe, so `parsed_destroy` can call it unconditionally.
pub unsafe fn dom_index_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(Box::from_raw(ptr as *mut DomIndex));
    }
}

/// The element index - the same object as the attr->owner index.
pub unsafe fn parsed_element_index(p: *mut Parsed) -> *mut c_void {
    ensure(p) as *mut c_void
}

/// The elements with tag id `tag_id`, in document order, or NULL with
/// `*count = 0`.
///
/// Taken as a FUNCTION POINTER by the XPath context, so the signature is fixed.
pub unsafe extern "C" fn element_index_tag(
    ptr: *const c_void,
    tag_id: usize,
    count: *mut usize,
) -> *const *mut LxbNode {
    let none = |count: *mut usize| -> *const *mut LxbNode {
        if !count.is_null() {
            *count = 0;
        }
        core::ptr::null()
    };

    if ptr.is_null() {
        return none(count);
    }
    let idx = &*(ptr as *const DomIndex);
    if idx.tag_nodes.is_empty()
        || tag_id == TAG_UNDEF
        || tag_id >= TAG_INDEX_CAP
        || tag_id > idx.tag_max
    {
        return none(count);
    }

    let start = idx.tag_off[tag_id];
    let end = idx.tag_off[tag_id + 1];
    if !count.is_null() {
        *count = end - start;
    }
    if end > start {
        idx.tag_nodes.as_ptr().add(start)
    } else {
        core::ptr::null()
    }
}

/// Whether the document holds any non-HTML element.
///
/// NULL answers 1 - assume foreign - which is the fail-safe direction: the
/// `//tag` fast path is only taken for a document known to be pure HTML.
pub unsafe extern "C" fn element_index_has_foreign(ptr: *const c_void) -> c_int {
    if ptr.is_null() {
        return 1;
    }
    c_int::from((*(ptr as *const DomIndex)).has_foreign)
}
