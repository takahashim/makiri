//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

use magnus::rb_sys::FromRawValue;
use magnus::{ExceptionClass, RModule, Value};
use rb_sys::VALUE;

/// An `lxb_dom_node_t`, opaque.
///
/// The glue never reads Lexbor's layout: every field it needs has an exported
/// accessor (`lxb_dom_node_type_noi` and the rest). That keeps this layer out of
/// the pinned-dependency layout problem that the XPath HTML backend has to
/// cross-check at load - there is nothing here to get wrong.
#[repr(C)]
pub struct LxbNode {
    _private: [u8; 0],
}

pub const LXB_STATUS_OK: u32 = 0x0000;
pub const LXB_STATUS_ERROR_MEMORY_ALLOCATION: u32 = 0x0002;

pub const LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT: u32 = 0x0B;

/// `LXB_HTML_SERIALIZE_OPT_UNDEF`.
pub const LXB_HTML_SERIALIZE_OPT_UNDEF: u32 = 0x00;

extern "C" {
    /* The module and exception `VALUE`s Init_makiri defines (makiri.c). */
    pub static mkr_mHtmlNodeMethods: VALUE;
    pub static mkr_eError: VALUE;

    /// The HTML node pointer behind a wrapper (glue/ruby_html_node.c).
    ///
    /// **Raises** (TypeError) for an XML node or a non-node, so see the
    /// longjmp rule in the module docs: call it before anything is live.
    pub fn mkr_html_node_unwrap(v: VALUE) -> *mut LxbNode;

    /// Live bytes in the node's document arena (dom_adapter/compat.h), which
    /// the serializers size their buffer from.
    pub fn mkr_lxb_document_bytes(node: *mut LxbNode) -> usize;

    pub fn lxb_dom_node_type_noi(node: *mut LxbNode) -> u32;
}

/// The `Makiri::HTML::NodeMethods` module every HTML node leaf includes.
///
/// # Safety
/// Only after `Init_makiri` has defined it, i.e. from a `mkr_init_*` or later.
pub unsafe fn html_node_methods() -> RModule {
    RModule::from_value(Value::from_raw(mkr_mHtmlNodeMethods))
        .expect("Makiri::HTML::NodeMethods is a Module")
}

/// `Makiri::Error`.
///
/// # Safety
/// As [`html_node_methods`].
pub unsafe fn error_class() -> ExceptionClass {
    ExceptionClass::from_value(Value::from_raw(mkr_eError))
        .expect("Makiri::Error is a Class < Exception")
}
