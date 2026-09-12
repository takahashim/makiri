//! The generated Lexbor layout, plus the agreement checks over it.
//!
//! `build.rs` runs bindgen over the vendored headers; this module includes the
//! result and pins the facts the rest of the crate depends on. See
//! notes/rust_port_remaining.ja.md step 5 for why generated rather than
//! hand-written, and for what this does not remove.

/// The generated bindings, with the blanket allows scoped to THEM.
///
/// They were on the whole module, which meant the hand-written parts below -
/// `consts`, the agreement checks - were also exempt from dead-code and naming
/// lints they should not be.
mod sys {
    #![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/lexbor_sys.rs"));
}

pub use sys::*;

/// Makiri's own C enums, generated for the same reason Lexbor's are - see
/// `generate_makiri_enums` in build.rs.
pub mod mkr {
    #![allow(non_camel_case_types, non_upper_case_globals, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/makiri_enums.rs"));
}

/* ------------------------------------------------------------------ *
 * Names                                                              *
 * ------------------------------------------------------------------ */

/* The DOM handle types, under the short names the rest of the crate uses. The
 * generated `lxb_dom_*_t` spellings stay available; these exist so a cast at a
 * call site reads as a cast to a Lexbor node rather than to a bindgen name. */
pub type LxbNode = lxb_dom_node_t;
pub type LxbElement = lxb_dom_element_t;
pub type LxbAttr = lxb_dom_attr_t;
pub type LxbDoc = lxb_dom_document_t;

/* ------------------------------------------------------------------ *
 * The `_noi` twins                                                   *
 * ------------------------------------------------------------------ */

/* The accessors Lexbor publishes only as `lxb_inline`, reached through the
 * `_noi` twin it exports for exactly this case.
 *
 * These are the one part of Lexbor's surface bindgen cannot supply: it does not
 * emit a `static inline`, so allowlisting the plain name yields nothing. That
 * absence is also how this list was *found* - each name below was allowlisted
 * in build.rs first, produced no binding, and only then was hand-declared here.
 * Guessing which ones are inline is what cost this project a segfault once
 * already: macOS links the extension with `-undefined dynamic_lookup`, so a
 * declaration that matches no symbol is not a link error, it is a NULL call at
 * run time. `rake symbols` is the standing check that none of these is missing.
 *
 * One declaration per symbol: everything in the crate reaches an `_noi` through
 * this module, never through its own extern block. */
extern "C" {
    pub fn lxb_dom_node_type_noi(node: *mut LxbNode) -> u32;
    pub fn lxb_dom_attr_value_noi(attr: *mut LxbAttr, len: *mut usize) -> *const u8;
    pub fn lxb_dom_element_first_attribute_noi(element: *mut LxbElement) -> *mut LxbAttr;
    pub fn lxb_dom_element_next_attribute_noi(attr: *mut LxbAttr) -> *mut LxbAttr;
    pub fn lxb_dom_document_type_public_id_noi(
        doctype: *mut lxb_dom_document_type_t,
        len: *mut usize,
    ) -> *const u8;
    pub fn lxb_dom_document_type_system_id_noi(
        doctype: *mut lxb_dom_document_type_t,
        len: *mut usize,
    ) -> *const u8;
    pub fn lxb_dom_processing_instruction_target_noi(
        pi: *mut lxb_dom_processing_instruction_t,
        len: *mut usize,
    ) -> *const u8;
    pub fn lxb_dom_document_destroy_text_noi(doc: *mut LxbDoc, text: *mut u8);
}

/* Exported by Lexbor but left out of its public headers, so bindgen cannot see
 * them either - the same hand-declared status as the `_noi` twins above, for a
 * different reason. Lexbor's own document_type.c forward-declares
 * `lxb_dom_attr_qualified_name_append` exactly this way.
 *
 * `lxb_ns_append` interns a namespace URI in the document's table.
 * `lxb_dom_attr_set_name_ns` names an attribute from (namespace, qualified
 * name), splitting prefix/local and interning the namespace.
 * `lxb_dom_attr_qualified_name_append` interns a name CASE-PRESERVING, which is
 * what lets createDocumentType keep its case where Lexbor's own create()
 * lowercases. */
extern "C" {
    pub fn lxb_ns_append(
        hash: *mut core::ffi::c_void,
        link: *const u8,
        length: usize,
    ) -> *const LxbNsData;
    pub fn lxb_dom_attr_set_name_ns(
        attr: *mut LxbAttr,
        link: *const u8,
        link_length: usize,
        name: *const u8,
        name_length: usize,
        to_lowercase: bool,
    ) -> u32;
    pub fn lxb_dom_attr_qualified_name_append(
        hash: *mut core::ffi::c_void,
        name: *const u8,
        length: usize,
    ) -> *mut LxbAttrData;
}

/// `lxb_ns_data_t`, of which only `ns_id` is read. It is `lexbor_hash_entry_t
/// entry; lxb_ns_id_t ns_id; ...`, and the entry's layout is Lexbor's business -
/// so the id is reached through the generated struct rather than guessed at.
pub type LxbNsData = lxb_ns_data_t;

/// `lxb_dom_attr_data_t`, likewise read only for `attr_id`.
pub type LxbAttrData = lxb_dom_attr_data_t;

/// The shared pre-order (document-order) walk over a subtree: the next node
/// after `node`, bounded to `root`, or NULL after the last one.
///
/// `mkr_dom_preorder_next` in C, where it is `static inline` in a header and so
/// has no symbol to call. It is written out here ONCE rather than per module:
/// the DoS-avoiding invariant it carries - climb via parent pointers, never
/// recurse, so an adversarially deep tree cannot exhaust the stack - is the
/// reason it exists, and two copies are two chances to lose it. The C header
/// says the same thing about its own single copy.
///
/// # Safety
/// `node` must be a live node in `root`'s subtree.
#[inline]
pub unsafe fn preorder_next(mut node: *mut LxbNode, root: *mut LxbNode) -> *mut LxbNode {
    if !(*node).first_child.is_null() {
        return (*node).first_child;
    }
    while node != root && (*node).next.is_null() {
        node = (*node).parent;
    }
    if node == root {
        return core::ptr::null_mut();
    }
    (*node).next
}

/// bindgen names an enum's constants by whether the enum is NAMED: a typedef'd
/// one gets its type as a prefix (`lxb_ns_id_enum_t_LXB_NS_HTML`), a truly
/// anonymous one keeps the bare name (`LXB_CSS_AT_RULE_MEDIA`). That is an
/// artefact of the headers, not something callers should have to know, so the
/// aliases below give every constant the name it has in C.
pub mod consts {
    /// Rule kinds (`lxb_css_rule_type_t`).
    pub const CSS_RULE_STYLE: usize = super::lxb_css_rule_type_t_LXB_CSS_RULE_STYLE as usize;
    pub const CSS_RULE_AT_RULE: usize = super::lxb_css_rule_type_t_LXB_CSS_RULE_AT_RULE as usize;
    pub const CSS_RULE_BAD_STYLE: usize =
        super::lxb_css_rule_type_t_LXB_CSS_RULE_BAD_STYLE as usize;
    pub const CSS_RULE_DECLARATION: usize =
        super::lxb_css_rule_type_t_LXB_CSS_RULE_DECLARATION as usize;

    /// At-rule kinds (an anonymous enum, hence the bare generated names).
    pub const AT_RULE_UNDEF: usize = super::LXB_CSS_AT_RULE__UNDEF as usize;
    pub const AT_RULE_CUSTOM: usize = super::LXB_CSS_AT_RULE__CUSTOM as usize;
    pub const AT_RULE_FONT_FACE: usize = super::LXB_CSS_AT_RULE_FONT_FACE as usize;
    pub const AT_RULE_MEDIA: usize = super::LXB_CSS_AT_RULE_MEDIA as usize;
    pub const AT_RULE_NAMESPACE: usize = super::LXB_CSS_AT_RULE_NAMESPACE as usize;
}

/* ------------------------------------------------------------------ *
 * Agreement with the hand-written view                               *
 * ------------------------------------------------------------------ */

/// `xpath/html_abi.rs` declares the three node structs by hand, and keeps doing
/// so for one reason: the engine's hot paths read those fields per node, and a
/// view shaped for that is worth having under the cursor rather than in
/// `OUT_DIR`. What it must not be is a SECOND source of truth, so every field
/// it declares is checked against the generated one here.
///
/// These are `const` assertions: a disagreement is a build failure, not a
/// load-time abort, and it names the field in the error. The C-side `offsetof`
/// check in `mkr_xpath_html_shim.c` stays for the one thing neither of these
/// covers - libclang and the build's `cc` disagreeing with each other.
#[cfg(feature = "xpath-html")]
mod agree {
    use super::{lxb_dom_attr_t, lxb_dom_element_t, lxb_dom_node_t};
    use crate::xpath::html_abi::{Attr, Element, Node};

    macro_rules! same_size {
        ($ours:ty, $theirs:ty, $what:literal) => {
            const _: () = assert!(
                core::mem::size_of::<$ours>() == core::mem::size_of::<$theirs>(),
                concat!("the hand-written ", $what, " has drifted from Lexbor's")
            );
        };
    }

    macro_rules! same_offset {
        ($ours:ty, $theirs:ty, $field:ident, $what:literal) => {
            const _: () = assert!(
                core::mem::offset_of!($ours, $field) == core::mem::offset_of!($theirs, $field),
                concat!($what, ".", stringify!($field), " is at a different offset")
            );
        };
    }

    same_size!(Node, lxb_dom_node_t, "lxb_dom_node_t");
    same_size!(Element, lxb_dom_element_t, "lxb_dom_element_t");
    same_size!(Attr, lxb_dom_attr_t, "lxb_dom_attr_t");

    // Every field the engine navigates by. Sizes alone would not catch a swap
    // of two same-typed fields, and `next`/`prev`/`parent` are exactly that
    // shape - getting them confused would walk the tree in the wrong direction
    // and still answer, which is the failure mode worth ruling out.
    same_offset!(Node, lxb_dom_node_t, local_name, "node");
    same_offset!(Node, lxb_dom_node_t, prefix, "node");
    same_offset!(Node, lxb_dom_node_t, ns, "node");
    same_offset!(Node, lxb_dom_node_t, owner_document, "node");
    same_offset!(Node, lxb_dom_node_t, next, "node");
    same_offset!(Node, lxb_dom_node_t, prev, "node");
    same_offset!(Node, lxb_dom_node_t, parent, "node");
    same_offset!(Node, lxb_dom_node_t, first_child, "node");
    same_offset!(Node, lxb_dom_node_t, last_child, "node");
    same_offset!(Node, lxb_dom_node_t, user, "node");
    same_offset!(Node, lxb_dom_node_t, type_, "node");

    same_offset!(Element, lxb_dom_element_t, first_attr, "element");
    same_offset!(Element, lxb_dom_element_t, last_attr, "element");

    same_offset!(Attr, lxb_dom_attr_t, owner, "attr");
    same_offset!(Attr, lxb_dom_attr_t, next, "attr");
    same_offset!(Attr, lxb_dom_attr_t, prev, "attr");
}

/* The namespace constants used to be hand-written in `xpath/html_abi.rs` and
 * checked here. They are now derived from the generated enum directly, so there
 * is nothing left to disagree - the check was removed rather than kept as
 * decoration. The layout checks above remain, because a hand-written struct
 * view is still what the engine's hot paths read. */
