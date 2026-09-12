//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

use core::ffi::{c_char, c_void};

use magnus::rb_sys::FromRawValue;
use magnus::{ExceptionClass, RModule, Value};
use rb_sys::VALUE;

/// `mkr_node_data_t` - what a node wrapper holds: the node pointer plus the
/// keepalive Document. Declared here because both `glue::node` (which owns the
/// TypedData) and `glue::xml_node` (which mints XML wrappers) write it.
#[repr(C)]
pub struct NodeData {
    /// `mkr_raw_node_t *` - representation-opaque; read it only through a
    /// kind-checked accessor.
    pub node: *mut c_void,
    pub document: VALUE,
}

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

/* Every symbol the glue shares with C is declared HERE, once.
 *
 * It used to be declared wherever it was needed, and that let two modules give
 * one symbol different types - `mkr_wrap_xml_node` as `*mut c_void` in one and
 * `*mut mkr_xml_node_t` in another. Rust rejects that, but only in a build where
 * both modules are present, so every single-feature CI leg passed and only the
 * "everything" leg failed. One declaration removes the possibility rather than
 * relying on that leg to notice.
 *
 * Node pointers cross this boundary as `c_void`: at the boundary a node IS
 * representation-opaque (the C calls it `mkr_raw_node_t`), and each caller casts
 * to the representation it has already established. */
extern "C" {
    /* The class, module and exception `VALUE`s Init_makiri defines (makiri.c). */
    pub static mkr_mHtmlNodeMethods: VALUE;
    pub static mkr_mXmlNodeMethods: VALUE;
    pub static mkr_mXML: VALUE;
    pub static mkr_cNode: VALUE;
    pub static mkr_cDocument: VALUE;
    pub static mkr_cNodeSet: VALUE;
    pub static mkr_cXmlDocument: VALUE;
    pub static mkr_cXmlDocumentFragment: VALUE;
    pub static mkr_eError: VALUE;
    pub static mkr_eCSSSyntaxError: VALUE;
    pub static mkr_eXmlSyntaxError: VALUE;
    pub static mkr_eXmlLimitExceeded: VALUE;

    /// The HTML node pointer behind a wrapper (glue/ruby_html_node.c).
    ///
    /// **Raises** (TypeError) for an XML node or a non-node, so see the
    /// longjmp rule in the module docs: call it before anything is live.
    pub fn mkr_html_node_unwrap(v: VALUE) -> *mut LxbNode;
    /// The XML counterpart; raises for an HTML node.
    pub fn mkr_xml_node_unwrap(v: VALUE) -> *mut c_void;

    /// Wrap a node into its Ruby leaf. NULL becomes nil, and a document node
    /// becomes the Document itself.
    pub fn mkr_wrap_html_node(node: *mut LxbNode, document: VALUE) -> VALUE;
    pub fn mkr_wrap_xml_node(node: *mut c_void, document: VALUE) -> VALUE;

    /// The keepalive Document of any wrapped node.
    pub fn mkr_node_document(rb_node: VALUE) -> VALUE;
    /// The kind-agnostic raw node pointer, for identity.
    pub fn mkr_node_raw(rb_node: VALUE) -> *mut c_void;

    pub fn mkr_node_set_new(document: VALUE) -> VALUE;
    pub fn mkr_node_set_push(set: VALUE, node: *mut c_void);

    /// The parsed-document handle behind a Document, and its XML arena.
    pub fn mkr_doc_parsed(rb_doc: VALUE) -> *mut c_void;
    pub fn mkr_parsed_xml_doc(p: *const c_void) -> *mut c_void;

    /// Enforce the strict text contract, naming `what`. **Raises.**
    pub fn mkr_verify_text(str: VALUE, what: *const c_char);

    /// Variadic, so callable but not definable from Rust. It longjmps, so no
    /// Rust destructor may be live at the call (see the module docs).
    pub fn rb_raise(exc: VALUE, fmt: *const c_char, ...) -> !;

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

/// The wrapped Rust value behind a TypedData object, without magnus's
/// `rb_protect`.
///
/// `<&T>::try_convert` - and so every magnus method with a wrapped receiver -
/// runs `rb_check_typeddata` inside `rb_protect`, which is a `setjmp` per call.
/// That is the right default when a Rust caller wants a `Result`, but it is not
/// free: on the per-node path it measured about a quarter of the throughput of
/// the C it replaced (`Node#css` over 2000 nodes, `notes/node_set_ab.rb`).
///
/// A C-ABI entry point wants the C behaviour anyway - `rb_check_typeddata`
/// raises `TypeError` on a mismatch, which is exactly what `TypedData_Get_Struct`
/// did - so it calls this instead.
///
/// # Safety
/// Raises (longjmps) when `v` is not a `T`, so no Rust destructor may be live.
/// The returned lifetime is unconstrained; the caller must keep `v` rooted.
pub unsafe fn typed_data_unprotected<'a, T: magnus::TypedData>(v: VALUE) -> &'a T {
    /* magnus::DataType is #[repr(transparent)] over rb_data_type_t, so this
     * cast is what the repr promises; the accessor for it is crate-private. */
    let dt = T::data_type() as *const magnus::typed_data::DataType as *const rb_sys::rb_data_type_t;
    &*(rb_sys::rb_check_typeddata(v, dt) as *const T)
}

/// `Makiri::Error`.
///
/// # Safety
/// As [`html_node_methods`].
pub unsafe fn error_class() -> ExceptionClass {
    ExceptionClass::from_value(Value::from_raw(mkr_eError))
        .expect("Makiri::Error is a Class < Exception")
}
