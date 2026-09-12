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
 * Generated, not restated. These were hand-written with a const-assert against
 * the header, on the reasoning that the hot comparisons should stay immediate
 * values - which a generated `pub const` also is, so the reasoning was simply
 * wrong and the assert was guarding a copy that need not have existed.
 *
 * The incident it was guarding against is real and worth remembering:
 * LXB_NS_HTML is 2, and a hand-written 1 made every HTML element foreign, so
 * every unprefixed name test matched nothing - silently. Deriving the value
 * removes the class rather than checking for it. */
pub const NS_UNDEF: usize = crate::lexbor_abi::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
pub const NS_HTML: usize = crate::lexbor_abi::lxb_ns_id_enum_t_LXB_NS_HTML as usize;

/// `LXB_TAG__UNDEF`. A custom element's tag id is a pointer value, far above
/// the static range the index buckets, so it is compared against
/// `TAG_LAST_ENTRY` rather than this.
pub const TAG_UNDEF: usize = crate::lexbor_abi::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize;

/* Lexbor's exported accessors, re-exported from the generated bindings rather
 * than declared again here.
 *
 * They WERE declared here, over the hand-written structs above, until
 * `glue/html_node` needed the same six and bindgen started emitting them: the
 * same C symbol then had two Rust types, which rustc reports as
 * "redeclared with a different signature". The structs stay hand-written for
 * the reason in `lexbor_abi::agree` - the engine reads their fields per node -
 * but a FUNCTION has no hot path to shape, so there is no reason for a second
 * declaration of one. The handles are cast at the call site, which is what
 * those call sites already did. */
pub use crate::lexbor_abi::{
    lxb_dom_attr_local_name, lxb_dom_attr_qualified_name, lxb_dom_element_get_attribute,
    lxb_dom_element_local_name, lxb_dom_element_qualified_name, lxb_dom_node_name, LxbAttr,
    LxbElement, LxbNode,
};

/// The `_noi` twin Lexbor publishes so a non-C caller can reach an
/// `lxb_inline` function. Declared once, in `lexbor_abi` - see the note there
/// for why that one is hand-written where the rest are generated.
pub use crate::lexbor_abi::lxb_dom_attr_value_noi;

extern "C" {
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
