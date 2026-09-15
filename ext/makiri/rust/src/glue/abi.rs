//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

use core::ffi::{c_char, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{ExceptionClass, RModule, Value};
use rb_sys::VALUE;

/// `mkr_node_data_t` - what a node wrapper holds: the node pointer plus the
/// keepalive Document. Declared here because both `glue::node` (which owns the
/// TypedData) and `glue::xml_node` (which mints XML wrappers) write it.
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
/// Now that `build.rs` generates Lexbor's layout, this IS that layout rather
/// than a second, opaque view of it. Modules that only pass the pointer along
/// are unaffected; the ones that read a field (glue::doc) get the real one, and
/// there is only one definition to be wrong.
pub type LxbNode = crate::lexbor_abi::lxb_dom_node_t;
/// `lxb_dom_document_t`. Shared vocabulary: both the Document wrapper and the
/// fragment pipeline pass it around.
pub type LxbDoc = crate::lexbor_abi::lxb_dom_document_t;

/* Every Lexbor constant below comes from the generated bindings, none is
 * transcribed. The names are re-exported here rather than used through
 * `lexbor_abi` at the call sites only because these particular ones are spelled
 * this way throughout the glue; `lexbor_abi::consts` is where a NEW one goes. */
pub const LXB_STATUS_OK: u32 = crate::lexbor_abi::lexbor_status_t_LXB_STATUS_OK;
pub const LXB_STATUS_ERROR_MEMORY_ALLOCATION: u32 =
    crate::lexbor_abi::lexbor_status_t_LXB_STATUS_ERROR_MEMORY_ALLOCATION;

pub const LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
pub const LXB_DOM_NODE_TYPE_ELEMENT: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
pub const LXB_DOM_NODE_TYPE_DOCUMENT_TYPE: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;

pub const LXB_HTML_SERIALIZE_OPT_UNDEF: u32 =
    crate::lexbor_abi::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF;

/* ------------------------------------------------------------------ *
 * The Ruby-string views, declared HERE, once                         *
 * ------------------------------------------------------------------ */

/* These were defined five times between them, and not always as the same C
 * type: `BorrowedText` meant the ANCHORED three-field `mkr_ruby_borrowed_text_t`
 * in four files and the unanchored two-field `mkr_verified_text_t` in a fifth.
 * Two distinct C types under one Rust name is worse than a duplicate - it is how
 * a caller reaches for the wrong one and gets a layout that happens to compile.
 * So the names below say which type they are, and the unanchored views live in
 * `crate::text` (`VerifiedText`, `BorrowedText`) rather than gaining another
 * alias. */

/// The contract a [`RubyStr`] was checked against. Uninhabited: types only.
pub enum TextContract {}
/// See [`TextContract`].
pub enum DataContract {}
/// See [`TextContract`].
pub enum BytesContract {}

/// Bytes borrowed from a Ruby String, together with the String that owns them.
///
/// The parameter records what was checked: [`RubyText`] is valid UTF-8 with no
/// NUL (`ptr` is NUL-terminated, so it also works as a C string), [`RubyData`]
/// is valid UTF-8 with NUL permitted (the HTML data family), and [`RubyBytes`]
/// is unchecked (HTML parsing decodes leniently). They are separate types
/// because the contract is the only thing that stops a data-family value from
/// reaching an engine input.
///
/// `Drop` is the keep-alive. It reads `value`, so the String stays visible to
/// the conservative stack scan until the guard goes out of scope - the C's
/// `RB_GC_GUARD` at the end of the borrow, without each call site having to
/// remember it. For a non-String argument that String is the coerced one, which
/// nothing else holds. Hence: not `Copy`, kept on the stack (never in a heap
/// container, which the GC does not scan), and read through `&self`.
///
/// Anchoring keeps the String alive and in place; it does not stop Ruby code
/// from mutating it. The bytes are read only while no Ruby code runs, which is
/// why reading them is `unsafe`.
pub struct RubyStr<C> {
    value: VALUE,
    ptr: *const c_char,
    len: usize,
    contract: core::marker::PhantomData<C>,
}

pub type RubyText = RubyStr<TextContract>;
pub type RubyData = RubyStr<DataContract>;
pub type RubyBytes = RubyStr<BytesContract>;

impl<C> RubyStr<C> {
    /// # Safety
    /// `ptr`/`len` must be the bytes of the String `value`, checked against `C`.
    pub(crate) unsafe fn from_raw_parts(value: VALUE, ptr: *const c_char, len: usize) -> Self {
        Self {
            value,
            ptr,
            len,
            contract: core::marker::PhantomData,
        }
    }

    /// No String at all: a null pointer, which Lexbor and the engine read as an
    /// omitted argument.
    pub(crate) fn absent() -> Self {
        Self {
            value: rb_sys::Qnil as VALUE,
            ptr: core::ptr::null(),
            len: 0,
            contract: core::marker::PhantomData,
        }
    }

    pub(crate) fn as_ptr(&self) -> *const c_char {
        self.ptr
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The bytes, or an empty slice when absent.
    ///
    /// # Safety
    /// No Ruby code may run, and so mutate the String, while the slice is used.
    pub(crate) unsafe fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.ptr as *const u8, self.len)
    }
}

impl RubyText {
    /// The bytes as an engine input.
    ///
    /// # Safety
    /// The view carries no lifetime: it must not be used after `self` drops, nor
    /// while Ruby code runs.
    pub(crate) unsafe fn as_verified(&self) -> crate::text::VerifiedText {
        // SAFETY: the bridge checked the text contract when it built `self`.
        unsafe { crate::text::VerifiedText::from_raw_parts(self.ptr, self.len) }
    }
}

impl<C> Drop for RubyStr<C> {
    fn drop(&mut self) {
        core::hint::black_box(self.value);
    }
}

/// `mkr_owned_bytes_t`: a heap buffer this side owns.
pub struct OwnedBytes {
    pub ptr: *mut c_char,
    pub len: usize,
}

impl OwnedBytes {
    pub const fn empty() -> OwnedBytes {
        OwnedBytes {
            ptr: core::ptr::null_mut(),
            len: 0,
        }
    }

    /// `mkr_owned_bytes_clear`, which is `static inline` in C and therefore has
    /// no symbol to call.
    ///
    /// # Safety
    /// `ptr` must be null or a live `malloc` allocation.
    pub unsafe fn clear(&mut self) {
        if !self.ptr.is_null() {
            crate::glue::abi::libc_free(self.ptr as *mut c_void);
        }
        self.ptr = core::ptr::null_mut();
        self.len = 0;
    }
}

/* Every symbol the glue shares with C is declared HERE, once.
 *
 * It used to be declared wherever it was needed, and that let two modules give
 * one symbol different types - `wrap_xml_node` as `*mut c_void` in one and
 * `*mut mkr_xml_node_t` in another. Rust rejects that, but only in a build where
 * both modules are present, so every single-feature CI leg passed and only the
 * "everything" leg failed. One declaration removes the possibility rather than
 * relying on that leg to notice.
 *
 * Node pointers cross this boundary as `c_void`: at the boundary a node IS
 * representation-opaque (the C calls it `mkr_raw_node_t`), and each caller casts
 * to the representation it has already established. */
pub use crate::bridge::string::ruby_bytes_view;
pub use crate::bridge::string::ruby_copy_bytes;
pub use crate::bridge::string::ruby_str_from_borrowed;
pub use crate::bridge::string::ruby_str_from_slices;
pub use crate::bridge::string::ruby_str_known_valid_utf8;
pub use crate::bridge::string::ruby_to_utf8;
pub use crate::bridge::string::ruby_verified_text;
pub use crate::bridge::string::verify_text;
pub use crate::dom_adapter::post_parse::lxb_document_bytes;
pub use crate::glue::doc::doc_parsed;
pub use crate::glue::doc::html_doc_unwrap;
pub use crate::glue::html_node::html_node_unwrap;
pub use crate::glue::html_node::wrap_html_node;
pub use crate::glue::node::keepalive_document;
pub use crate::glue::node::node_raw;
pub use crate::glue::node_set::node_set_new;
pub use crate::glue::node_set::node_set_push;
pub use crate::glue::xml_node::wrap_xml_node;
pub use crate::glue::xml_node::xml_node_unwrap;
pub use crate::init::CLASS_DOCUMENT;
pub use crate::init::CLASS_DOCUMENT_FRAGMENT;
pub use crate::init::CLASS_HTML_DOCUMENT;
pub use crate::init::CLASS_NODE;
pub use crate::init::CLASS_NODE_SET;
pub use crate::init::CLASS_XML_DOCUMENT;
pub use crate::init::CLASS_XML_DOCUMENT_FRAGMENT;
pub use crate::init::EXC_CSS_SYNTAX_ERROR;
pub use crate::init::EXC_ERROR;
pub use crate::init::EXC_XML_LIMIT_EXCEEDED;
pub use crate::init::EXC_XML_SYNTAX_ERROR;
pub use crate::init::MOD_HTML_NODE_METHODS;
pub use crate::init::MOD_LEXBOR;
pub use crate::init::MOD_XML;
pub use crate::init::MOD_XML_NODE_METHODS;

/// The XML arena behind a parsed handle, or null for an HTML one.
///
/// # Safety
/// `p` must be a live handle.
pub unsafe fn parsed_xml_doc(
    p: *mut crate::dom_adapter::post_parse::Parsed,
) -> *mut crate::xml::model::Document {
    (*p).xml_doc()
}

extern "C" {

    /// libc `free`, for buffers C handed us that C's own allocator owns.
    #[link_name = "free"]
    pub fn libc_free(p: *mut c_void);

    /// Variadic, so callable but not definable from Rust. It longjmps, so no
    /// Rust destructor may be live at the call (see the module docs).
    pub fn rb_raise(exc: VALUE, fmt: *const c_char, ...) -> !;
}

/// Lexbor's `lxb_inline` accessors, through the `_noi` twins it exports. They
/// live in `lexbor_abi` - the one place in the crate that hand-declares a Lexbor
/// function, because bindgen cannot generate an inline one - and are re-exported
/// here so this module stays the single import for the glue layer.
pub use crate::lexbor_abi::{
    lxb_dom_attr_value_noi, lxb_dom_document_destroy_text_noi, lxb_dom_document_type_public_id_noi,
    lxb_dom_document_type_system_id_noi, lxb_dom_element_first_attribute_noi,
    lxb_dom_element_next_attribute_noi, lxb_dom_node_type_noi,
    lxb_dom_processing_instruction_target_noi,
};

/// The generated Lexbor readers the glue calls, likewise re-exported so a glue
/// file imports one module.
pub use crate::lexbor_abi::{
    lxb_dom_attr_local_name, lxb_dom_attr_qualified_name, lxb_dom_document_root,
    lxb_dom_element_get_attribute, lxb_dom_element_has_attribute, lxb_dom_element_local_name,
    lxb_dom_element_qualified_name, lxb_dom_element_tag_name, lxb_dom_node_name,
    lxb_dom_node_text_content, lxb_ns_by_id, LxbAttr, LxbElement,
};

/// The `Makiri::HTML::NodeMethods` module every HTML node leaf includes.
///
/// # Safety
/// Only after `Init_makiri` has defined it, i.e. from a `mkr_init_*` or later.
pub unsafe fn html_node_methods() -> RModule {
    RModule::from_value(Value::from_raw(MOD_HTML_NODE_METHODS))
        .expect("Makiri::HTML::NodeMethods is a Module")
}

/// Is `v` an instance of the class held in `klass`?
///
/// # Safety
/// `klass` must hold a live Class (one of the statics above).
pub unsafe fn is_kind_of(v: Value, klass: VALUE) -> bool {
    rb_sys::rb_obj_is_kind_of(v.as_raw(), klass) == rb_sys::Qtrue as VALUE
}

pub use crate::bridge::ruby::typed_data_unprotected;

/// `Makiri::Error`.
///
/// # Safety
/// As [`html_node_methods`].
pub unsafe fn error_class() -> ExceptionClass {
    ExceptionClass::from_value(Value::from_raw(EXC_ERROR))
        .expect("Makiri::Error is a Class < Exception")
}

/* ------------------------------------------------------------------ *
 * Lexbor's CSS parser, declared once                                 *
 * ------------------------------------------------------------------ */

/// Opaque: neither user reads a field of it.
///
/// Three users need this parser - the selector engine, the stylesheet binding,
/// and the CSS lowering, which is Ruby-free and so needs it without magnus. The
/// declaration lives in `lexbor_abi` and all three re-export it from there.
/// Giving one C symbol two Rust types is the failure this file exists to
/// prevent; it has happened twice (wrap_xml_node, and again while the
/// stylesheet binding was written). Both escaped until an "everything" build
/// compiled the two definitions together - which every build now is, so a
/// second definition is a build error rather than something a feature
/// combination has to go looking for.
pub use crate::lexbor_abi::{
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    CssParser,
};

/* ------------------------------------------------------------------ *
 * rb_data_type_t in a static                                         *
 * ------------------------------------------------------------------ */

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
pub struct DataType(rb_sys::rb_data_type_t);

// SAFETY: the contents are set once at compile time and never mutated. Ruby
// reads them from whichever thread holds the GVL.
unsafe impl Sync for DataType {}

impl DataType {
    /// `parent` is null for a base type.
    pub const fn new(
        name: *const core::ffi::c_char,
        parent: *const rb_sys::rb_data_type_t,
        dmark: rb_sys::RUBY_DATA_FUNC,
        dfree: rb_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> rb_sys::size_t>,
    ) -> DataType {
        DataType(rb_sys::rb_data_type_t {
            wrap_struct_name: name,
            function: rb_sys::rb_data_type_struct__bindgen_ty_1 {
                dmark,
                dfree,
                dsize,
                dcompact: None,
                reserved: [core::ptr::null_mut(); 1],
            },
            parent,
            data: core::ptr::null_mut(),
            flags: rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE,
        })
    }

    /// The raw pointer the Ruby API wants.
    #[inline]
    pub const fn as_ptr(&self) -> *const rb_sys::rb_data_type_t {
        self as *const DataType as *const rb_sys::rb_data_type_t
    }
}

/* ------------------------------------------------------------------ *
 * declaration/definition agreement                                   *
 * ------------------------------------------------------------------ */

/// Some symbols declared above are DEFINED by this crate when the feature that
/// ports their C file is on. rustc does not check a `#[no_mangle]` definition
/// against an `extern` block - the two are separate items - so one symbol could
/// get two types again, silently, which is the exact failure this file exists to
/// prevent.
///
/// Coercing each definition to the declared function type closes that: a
/// mismatch is a build error naming the symbol. It costs nothing at runtime.
mod agree {
    #![allow(unused_imports)]
    use super::*;

    /// The constant is NAMED after the symbol, so a mismatch reads
    /// `const doc_parsed: unsafe extern "C" fn(...)` and says which one.
    /// The first version took the name and never used it: the check was real,
    /// the message was a line number, and the commit that added it claimed the
    /// symbol was named. Naming it is the whole point of having the check say
    /// anything at all.
    macro_rules! same_signature {
        ($sym:ident, $path:path, $ty:ty) => {
            #[allow(non_upper_case_globals, dead_code)]
            const $sym: $ty = {
                let f: $ty = $path as $ty;
                f
            };
        };
    }

    same_signature!(
        doc_parsed,
        crate::glue::doc::doc_parsed,
        unsafe fn(VALUE) -> Result<*mut crate::dom_adapter::post_parse::Parsed, magnus::Error>
    );
    same_signature!(
        html_doc_unwrap,
        crate::glue::doc::html_doc_unwrap,
        unsafe fn(VALUE) -> Result<*mut crate::lexbor_abi::LxbDoc, magnus::Error>
    );
    same_signature!(
        wrap_document,
        crate::glue::doc::wrap_document,
        unsafe extern "C" fn(*mut crate::dom_adapter::post_parse::Parsed) -> VALUE
    );

    same_signature!(
        keepalive_document,
        crate::glue::node::keepalive_document,
        unsafe fn(VALUE) -> Result<VALUE, magnus::Error>
    );
    same_signature!(
        node_raw,
        crate::glue::node::node_raw,
        unsafe fn(VALUE) -> Result<*mut c_void, magnus::Error>
    );

    same_signature!(
        node_set_new,
        crate::glue::node_set::node_set_new,
        unsafe extern "C" fn(VALUE) -> VALUE
    );
    same_signature!(
        node_set_push,
        crate::glue::node_set::node_set_push,
        unsafe extern "C" fn(VALUE, *mut c_void)
    );

    same_signature!(
        verify_text,
        crate::bridge::string::verify_text,
        unsafe fn(VALUE, *const c_char) -> Result<(), magnus::Error>
    );
    same_signature!(
        ruby_verified_text,
        crate::bridge::string::ruby_verified_text,
        unsafe fn(VALUE, *const c_char) -> Result<RubyText, magnus::Error>
    );

    same_signature!(
        xml_node_unwrap,
        crate::glue::xml_node::xml_node_unwrap,
        unsafe fn(VALUE) -> Result<*mut c_void, magnus::Error>
    );

    same_signature!(
        wrap_html_node,
        crate::glue::html_node::wrap_html_node,
        unsafe extern "C" fn(*mut LxbNode, VALUE) -> VALUE
    );
    same_signature!(
        html_node_unwrap,
        crate::glue::html_node::html_node_unwrap,
        unsafe fn(VALUE) -> Result<*mut LxbNode, magnus::Error>
    );
}
