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
    #![allow(
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals,
        dead_code
    )]
    include!(concat!(env!("OUT_DIR"), "/lexbor_sys.rs"));
}

pub use sys::*;

/// Makiri's own constants, and the layout of the handle a parse returns.
///
/// Unlike everything above, these are NOT generated. They were, from
/// `ext/makiri/*.h` - added after a transcribed `MKR_NODE_KIND_XML = 1` (it is
/// 2) made `Document#import_node` treat every HTML node as an XML one. Those
/// headers went with the rest of the C, so there is no second reading of them
/// left to check against: this module is now the definition. That is why the
/// field order below is spelled out rather than left to be noticed.
pub mod mkr {
    #![allow(non_camel_case_types, non_upper_case_globals, dead_code)]
    use core::ffi::{c_uint, c_void};

    /// Which representation a wrapped Ruby node is, by its TypedData type (NOT
    /// by Ruby class). A Document, a NodeSet, or any non-node is OTHER.
    pub type mkr_node_kind_t = c_uint;
    pub const mkr_node_kind_t_MKR_NODE_KIND_OTHER: mkr_node_kind_t = 0;
    pub const mkr_node_kind_t_MKR_NODE_KIND_HTML: mkr_node_kind_t = 1;
    pub const mkr_node_kind_t_MKR_NODE_KIND_XML: mkr_node_kind_t = 2;

    /// The document kind a `mkr_parsed_t` holds. HTML points `doc` at a Lexbor
    /// `lxb_html_document_t`; XML points it at our own arena.
    pub type mkr_doc_kind_t = c_uint;
    pub const mkr_doc_kind_t_MKR_DOC_HTML: mkr_doc_kind_t = 0;
    pub const mkr_doc_kind_t_MKR_DOC_XML: mkr_doc_kind_t = 1;

    /// The result of a parse. Owns the document arena (Lexbor for HTML, ours
    /// for XML). The three indices are HTML-only, created lazily and null until
    /// then; the destroy path frees whatever is set.
    ///
    /// The field ORDER is load-bearing - `doc` first, `kind` second - because
    /// four `dom_adapter` modules read the first two by offset. It was
    /// generated for exactly that reason; keeping the layout stated once, here,
    /// is what replaces the generation.
    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct mkr_parsed_s {
        /// HTML: `lxb_html_document_t*` / XML: `mkr_xml_doc_t*`.
        pub doc: *mut c_void,
        pub kind: mkr_doc_kind_t,
        /// attr->owner map + the tag->elements index.
        pub dom_index: *mut c_void,
        /// byte offset -> source line.
        pub newline_idx: *mut c_void,
        /// node -> descendant-text slice run.
        pub text_index: *mut c_void,
    }
    pub type mkr_parsed_t = mkr_parsed_s;
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
    pub fn lxb_tag_id_by_name_noi(
        hash: *mut lexbor_hash_t,
        name: *const lxb_char_t,
        len: usize,
    ) -> lxb_tag_id_t;

    /* The tokenizer accessors the source-location recorder needs. Lexbor has a
     * setter and a ctx getter for the token-done callback but NO getter for the
     * callback function itself, so that one field is read directly from the
     * generated struct - see `dom_adapter::source_loc`. */
    pub fn lxb_html_parser_tokenizer_noi(
        parser: *mut lxb_html_parser_t,
    ) -> *mut lxb_html_tokenizer_t;
    pub fn lxb_html_tokenizer_callback_token_done_set_noi(
        tkz: *mut lxb_html_tokenizer_t,
        cb: lxb_html_tokenizer_token_f,
        ctx: *mut core::ffi::c_void,
    );
    pub fn lxb_html_tokenizer_callback_token_done_ctx_noi(
        tkz: *mut lxb_html_tokenizer_t,
    ) -> *mut core::ffi::c_void;
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
 * The CSS selector parser                                            *
 * ------------------------------------------------------------------ *
 *
 * Opaque, because neither caller reads a field - they hold pointers and call
 * accessors. Declared HERE rather than in `glue::abi`, where they started, so
 * the Ruby-free CSS lowering (`crate::css`) can reach them: that module is
 * compiled once for both engine instances and must not pull in magnus. */

/// `lxb_css_parser_t`.
#[repr(C)]
pub struct CssParser {
    _private: [u8; 0],
}

/// `lxb_css_memory_t`.
#[repr(C)]
pub struct CssMemory {
    _private: [u8; 0],
}

/// `lxb_css_selectors_t`.
#[repr(C)]
pub struct CssSelectors {
    _private: [u8; 0],
}

extern "C" {
    pub fn lxb_css_parser_create() -> *mut CssParser;
    pub fn lxb_css_parser_init(parser: *mut CssParser, tkz: *mut core::ffi::c_void) -> u32;
    pub fn lxb_css_parser_clean(parser: *mut CssParser);
    pub fn lxb_css_parser_destroy(parser: *mut CssParser, self_destroy: bool) -> *mut CssParser;

    pub fn lxb_css_memory_create() -> *mut CssMemory;
    pub fn lxb_css_memory_init(mem: *mut CssMemory, prepare_count: usize) -> u32;
    pub fn lxb_css_memory_clean(mem: *mut CssMemory);
    pub fn lxb_css_memory_destroy(mem: *mut CssMemory, self_destroy: bool) -> *mut CssMemory;

    pub fn lxb_css_selectors_create() -> *mut CssSelectors;
    pub fn lxb_css_selectors_init(sel: *mut CssSelectors) -> u32;
    pub fn lxb_css_selectors_destroy(
        sel: *mut CssSelectors,
        self_destroy: bool,
    ) -> *mut CssSelectors;

    pub fn lxb_css_selectors_parse(
        parser: *mut CssParser,
        data: *const u8,
        length: usize,
    ) -> *mut lxb_css_selector_list_t;

    /* The `lxb_inline` accessors, through their `_noi` twins. */
    pub fn lxb_css_parser_status_noi(parser: *mut CssParser) -> u32;
    pub fn lxb_css_parser_memory_set_noi(parser: *mut CssParser, mem: *mut CssMemory);
    pub fn lxb_css_parser_selectors_set_noi(parser: *mut CssParser, sel: *mut CssSelectors);
}

/* ------------------------------------------------------------------ *
 * Agreement with the hand-written view                               *
 * ------------------------------------------------------------------ */

/// `xpath/lexbor_abi.rs` declares the three node structs by hand, and keeps doing
/// so for one reason: the engine's hot paths read those fields per node, and a
/// view shaped for that is worth having under the cursor rather than in
/// `OUT_DIR`. What it must not be is a SECOND source of truth, so every field
/// it declares is checked against the generated one here.
///
/// These are `const` assertions: a disagreement is a build failure, not a
/// load-time abort, and it names the field in the error. The C-side `offsetof`
/// check in `mkr_xpath_html_shim.c` stays for the one thing neither of these
/// covers - libclang and the build's `cc` disagreeing with each other.
mod agree {
    use super::{lxb_dom_attr_t, lxb_dom_element_t, lxb_dom_node_t};
    use crate::xpath::lexbor_abi::{Attr, Element, Node};

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

    // The engine casts a node handle to an element or attr handle, which is
    // sound only while the node sits FIRST in both. That is a claim about the
    // absolute offset, not about the two views agreeing, so it is asserted as
    // zero rather than as a match - if Lexbor ever put a field ahead of `node`,
    // both sides would move together and an agreement check would still pass
    // while every cast in the engine became wrong.
    //
    // mkr_xpath_html_shim.c's runtime checker asserted these two (as
    // "element.node offset (0)" / "attr.node offset (0)") and nothing else did.
    // They moved here when it was retired, so retiring it lost no coverage.
    const _: () = assert!(
        core::mem::offset_of!(lxb_dom_element_t, node) == 0,
        "lxb_dom_element_t no longer starts with its node - the handle cast is unsound"
    );
    const _: () = assert!(
        core::mem::offset_of!(lxb_dom_attr_t, node) == 0,
        "lxb_dom_attr_t no longer starts with its node - the handle cast is unsound"
    );
    same_offset!(Element, lxb_dom_element_t, node, "element");
    same_offset!(Attr, lxb_dom_attr_t, node, "attr");
}

/* The namespace constants used to be hand-written in `xpath/lexbor_abi.rs` and
 * checked here. They are now derived from the generated enum directly, so there
 * is nothing left to disagree - the check was removed rather than kept as
 * decoration. The layout checks above remain, because a hand-written struct
 * view is still what the engine's hot paths read. */
