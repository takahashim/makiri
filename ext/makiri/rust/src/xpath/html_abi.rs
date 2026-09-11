//! Lexbor's node / element / attr structs as the HTML backend reads them, and
//! the Lexbor entry points it calls.
//!
//! Unlike `mkr_xml_node_t`, these belong to a vendored dependency whose pin
//! moves (CLAUDE.md), so a reordered field would not fail to build - it would
//! read the wrong offset. `layout_facts` below reports what this file believes,
//! and mkr_xpath_html_shim.c compares every one against the real `offsetof`
//! before anything can run a query.
//!
//! Only the fields the engine navigates by are declared. Everything else Lexbor
//! offers goes through its exported functions - including the two it publishes
//! as `lxb_inline`, which it also exports as `_noi` for exactly this case.

use super::abi::Buf;
use core::ffi::{c_char, c_int, c_void};

/// `lxb_dom_node_t`. 96 bytes; the first field is an event-target pointer.
#[repr(C)]
pub struct Node {
    pub events: *mut c_void,
    /// interned, lowercase, without prefix
    pub local_name: usize,
    pub prefix: usize,
    /// namespace id; 0 is LXB_NS__UNDEF
    pub ns: usize,
    pub owner_document: *mut Document,
    pub next: *mut Node,
    pub prev: *mut Node,
    pub parent: *mut Node,
    pub first_child: *mut Node,
    pub last_child: *mut Node,
    /// Reserved by Makiri for a source-location byte offset - do not read it
    /// here, and never write it.
    pub user: *mut c_void,
    pub type_: u32,
}

/// `lxb_dom_element_t`. The engine only navigates to `first_attr`; the fields
/// before it are declared to place it, not to be read.
#[repr(C)]
pub struct Element {
    pub node: Node,
    pub upper_name: usize,
    pub qualified_name: usize,
    pub is_value: *mut c_void,
    pub first_attr: *mut Attr,
    pub last_attr: *mut Attr,
    pub attr_id: *mut Attr,
    pub attr_class: *mut Attr,
    pub style: *mut c_void,
    pub list: *mut c_void,
    /* Two C enums, so 4 bytes each - not pointer-sized like the ids above. */
    pub condition: u32,
    pub custom_state: u32,
}

/// `lxb_dom_attr_t`. Attributes hang off an element in their own list, so the
/// attribute axis walks `next` here rather than the node's.
#[repr(C)]
pub struct Attr {
    pub node: Node,
    pub upper_name: usize,
    pub qualified_name: usize,
    pub value: *mut c_void,
    pub owner: *mut Element,
    pub next: *mut Attr,
    pub prev: *mut Attr,
}

/// `lxb_dom_document_t`, opaque: the engine reaches into it only through the
/// two shims, which see the real header.
#[repr(C)]
pub struct Document {
    _private: [u8; 0],
}

/* ---- the Lexbor constants the engine compares against ----
 *
 * Restated here rather than read from C so the hot comparisons stay immediate
 * values, and checked below for the same reason the offsets are: a generated
 * constant that moves with the pin would otherwise change the answer silently.
 * (It already did once - LXB_NS_HTML is 2, and guessing 1 made every HTML
 * element foreign, so every unprefixed name test matched nothing.) */
pub const NS_UNDEF: usize = 0x00;
pub const NS_HTML: usize = 0x02;

/// `LXB_TAG__UNDEF`. A custom element's tag id is a pointer value, far above
/// the static range the index buckets, so it is compared against
/// `TAG_LAST_ENTRY` rather than this.
pub const TAG_UNDEF: usize = 0;

extern "C" {
    /* Lexbor's exported accessors. */
    pub fn lxb_dom_element_local_name(el: *const Element, len: *mut usize) -> *const u8;
    pub fn lxb_dom_element_qualified_name(el: *const Element, len: *mut usize) -> *const u8;
    pub fn lxb_dom_attr_local_name(a: *const Attr, len: *mut usize) -> *const u8;
    pub fn lxb_dom_attr_qualified_name(a: *const Attr, len: *mut usize) -> *const u8;
    pub fn lxb_dom_node_name(n: *mut Node, len: *mut usize) -> *const u8;
    pub fn lxb_dom_element_get_attribute(
        el: *mut Element,
        qualified_name: *const u8,
        qn_len: usize,
        value_len: *mut usize,
    ) -> *const u8;
    /// The `_noi` twin Lexbor publishes so a non-C caller can reach an
    /// `lxb_inline` function.
    pub fn lxb_dom_attr_value_noi(a: *mut Attr, len: *mut usize) -> *const u8;

    /* Our shims (mkr_xpath_html_shim.c). */
    pub fn mkr_html_ns_uri(
        node: *const Node,
        doc: *const Document,
        len: *mut usize,
    ) -> *const c_char;
    pub fn mkr_html_tag_id_by_name(doc: *const Document, p: *const c_char, len: usize) -> usize;
    pub fn mkr_html_append_own_text(node: *mut Node, buf: *mut Buf) -> c_int;
}

extern "C" {
    /// `LXB_TAG__LAST_ENTRY` - the end of Lexbor's static tag-id range, read
    /// from C so this file does not restate a generated constant.
    #[link_name = "mkr_html_tag_last_entry"]
    pub static TAG_LAST_ENTRY: usize;
}

/// What this file believes Lexbor's layout is. The C side checks every one; see
/// the module header for why a build failure is not available here.
///
/// # Safety
/// `out` must be NULL or name `cap` writable `size_t`.
#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_rs_html_layout(out: *mut usize, cap: usize) -> usize {
    use core::mem::{offset_of, size_of};
    let facts = [
        size_of::<Node>(),
        offset_of!(Node, ns),
        offset_of!(Node, owner_document),
        offset_of!(Node, next),
        offset_of!(Node, prev),
        offset_of!(Node, parent),
        offset_of!(Node, first_child),
        offset_of!(Node, last_child),
        offset_of!(Node, type_),
        size_of::<Element>(),
        offset_of!(Element, first_attr),
        size_of::<Attr>(),
        offset_of!(Attr, next),
        offset_of!(Element, node),
        offset_of!(Attr, node),
        NS_UNDEF,
        NS_HTML,
        TAG_UNDEF,
    ];
    if !out.is_null() {
        for (i, f) in facts.iter().enumerate().take(cap) {
            *out.add(i) = *f;
        }
    }
    facts.len()
}
