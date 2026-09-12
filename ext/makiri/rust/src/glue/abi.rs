//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

use core::ffi::{c_int, c_char, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
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
/// Now that `build.rs` generates Lexbor's layout, this IS that layout rather
/// than a second, opaque view of it. Modules that only pass the pointer along
/// are unaffected; the ones that read a field (glue::doc) get the real one, and
/// there is only one definition to be wrong.
pub type LxbNode = crate::lexbor_abi::lxb_dom_node_t;

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

pub const LXB_HTML_SERIALIZE_OPT_UNDEF: u32 = crate::lexbor_abi::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF;

/* ------------------------------------------------------------------ *
 * The Ruby-string views, declared HERE, once                         *
 * ------------------------------------------------------------------ */

/* These were defined five times between them, and not always as the same C
 * type: `BorrowedText` meant the ANCHORED three-field `mkr_ruby_borrowed_text_t`
 * in four files and the unanchored two-field `mkr_verified_text_t` in a fifth.
 * Two distinct C types under one Rust name is worse than a duplicate - it is how
 * a caller reaches for the wrong one and gets a layout that happens to compile.
 * So the names below say which C type they are, and the unanchored one keeps its
 * existing home in `crate::xpath_abi::VerifiedText` rather than gaining a
 * fourth alias. */

/// `mkr_ruby_borrowed_text_t`: bytes borrowed from a Ruby String, with the
/// String itself so it stays alive while the view is on the stack.
///
/// The contract is "valid UTF-8, no NUL" and `ptr` is NUL-terminated, so it also
/// works as a C string. Layout-identical to [`RubyBytes`]; they are separate
/// types because the CONTRACT differs, which is the only thing that stops a
/// data-family value from reaching an engine input.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RubyText {
    pub value: VALUE,
    pub ptr: *const c_char,
    pub len: usize,
}

impl RubyText {
    /// The bytes, or an empty slice when absent.
    ///
    /// # Safety
    /// Valid only while the anchoring String is live and Ruby has not run.
    pub unsafe fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.ptr as *const u8, self.len)
    }
}


/// Dropping the Ruby anchor: the engine takes the unanchored form, and the
/// caller is responsible for keeping the String alive across the call.
impl From<RubyText> for crate::xpath_abi::VerifiedText {
    fn from(b: RubyText) -> Self {
        crate::xpath_abi::VerifiedText { ptr: b.ptr, len: b.len }
    }
}

/// `mkr_ruby_borrowed_bytes_t`: the same shape as [`RubyText`] with a weaker
/// contract - any bytes, not necessarily UTF-8 or NUL-free.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RubyBytes {
    pub value: VALUE,
    pub ptr: *const c_char,
    pub len: usize,
}

impl RubyBytes {
    /// # Safety
    /// Valid only while the anchoring String is live and Ruby has not run.
    pub unsafe fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.ptr as *const u8, self.len)
    }
}

/// `mkr_owned_bytes_t`: a heap buffer this side owns.
#[repr(C)]
pub struct OwnedBytes {
    pub ptr: *mut c_char,
    pub len: usize,
}

impl OwnedBytes {
    pub const fn empty() -> OwnedBytes {
        OwnedBytes { ptr: core::ptr::null_mut(), len: 0 }
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
    /// libc `free`, for buffers C handed us that C's own allocator owns.
    #[link_name = "free"]
    pub fn libc_free(p: *mut c_void);

    /* The class, module and exception `VALUE`s Init_makiri defines (makiri.c). */
    pub static mkr_mHtmlNodeMethods: VALUE;
    pub static mkr_mXmlNodeMethods: VALUE;
    pub static mkr_mXML: VALUE;
    /// `Makiri::Lexbor` (makiri.c). The CSS stylesheet binding hangs off it.
    pub static mkr_mLexbor: VALUE;
    pub static mkr_cNode: VALUE;
    pub static mkr_cDocument: VALUE;
    pub static mkr_cNodeSet: VALUE;
    pub static mkr_cXmlDocument: VALUE;
    pub static mkr_cXmlDocumentFragment: VALUE;
    pub static mkr_cHtmlDocument: VALUE;
    pub static mkr_cDocumentFragment: VALUE;
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
    /// The same contract, returning the anchored view. **Raises.**
    pub fn mkr_ruby_verified_text(input: VALUE, what: *const c_char) -> RubyText;

    /* The text-input contract's other half (bridge/ruby_string.c): honour the
     * String's encoding, and read its cached coderange without forcing a scan.
     * Both the document parse and the fragment decode need them. */
    pub fn mkr_ruby_to_utf8(v: VALUE) -> VALUE;
    pub fn mkr_ruby_str_known_valid_utf8(v: VALUE) -> bool;
    pub fn mkr_ruby_bytes_view(v: VALUE) -> RubyBytes;
    pub fn mkr_ruby_copy_bytes(v: VALUE, out: *mut OwnedBytes) -> c_int;

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

/// Is `v` an instance of the class held in `klass`?
///
/// # Safety
/// `klass` must hold a live Class (one of the statics above).
pub unsafe fn is_kind_of(v: Value, klass: VALUE) -> bool {
    rb_sys::rb_obj_is_kind_of(v.as_raw(), klass) == rb_sys::Qtrue as VALUE
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

/* ------------------------------------------------------------------ *
 * Lexbor's CSS parser, declared once                                 *
 * ------------------------------------------------------------------ */

/// Opaque: neither user reads a field of it.
///
/// Two independent features need this parser - `glue-css` for the selector
/// engine and `glue-lexbor-css` for the stylesheet binding - and they are not
/// feature-dependent on each other, so the declaration lives here rather than
/// in either. Giving one C symbol two Rust types is the failure this file
/// exists to prevent; it has happened before (mkr_wrap_xml_node) and happened
/// again while the stylesheet binding was being written, caught only by the
/// "everything" feature combination.
#[repr(C)]
pub struct CssParser {
    _private: [u8; 0],
}

extern "C" {
    pub fn lxb_css_parser_create() -> *mut CssParser;
    pub fn lxb_css_parser_init(parser: *mut CssParser, tkz: *mut core::ffi::c_void) -> u32;
    pub fn lxb_css_parser_clean(parser: *mut CssParser);
    pub fn lxb_css_parser_destroy(parser: *mut CssParser, self_destroy: bool) -> *mut CssParser;
}

/* ------------------------------------------------------------------ *
 * rb_data_type_t in a static                                         *
 * ------------------------------------------------------------------ */

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; the C originals are
/// `const` at file scope and are equally shared. `repr(transparent)` keeps the
/// exported symbol's layout exactly `rb_data_type_t`, which is what the C
/// `extern` declarations in glue.h expect.
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
    /// `const mkr_doc_parsed: unsafe extern "C" fn(...)` and says which one.
    /// The first version took the name and never used it: the check was real,
    /// the message was a line number, and the commit that added it claimed the
    /// symbol was named. Naming it is the whole point of having the check say
    /// anything at all.
    macro_rules! same_signature {
        ($sym:ident, $path:path, $ty:ty) => {
            #[allow(non_upper_case_globals, dead_code)]
            const $sym: $ty = {
                let f: $ty = $path;
                f
            };
        };
    }

    #[cfg(feature = "glue-doc")]
    same_signature!(
        mkr_doc_parsed,
        crate::glue::doc::mkr_doc_parsed,
        unsafe extern "C" fn(VALUE) -> *mut c_void
    );
    #[cfg(feature = "glue-doc")]
    same_signature!(
        mkr_wrap_document,
        crate::glue::doc::mkr_wrap_document,
        unsafe extern "C" fn(*mut c_void) -> VALUE
    );

    #[cfg(feature = "glue-node")]
    same_signature!(
        mkr_node_document,
        crate::glue::node::mkr_node_document,
        unsafe extern "C" fn(VALUE) -> VALUE
    );
    #[cfg(feature = "glue-node")]
    same_signature!(
        mkr_node_raw,
        crate::glue::node::mkr_node_raw,
        unsafe extern "C" fn(VALUE) -> *mut c_void
    );

    #[cfg(feature = "glue-node-set")]
    same_signature!(
        mkr_node_set_new,
        crate::glue::node_set::mkr_node_set_new,
        unsafe extern "C" fn(VALUE) -> VALUE
    );
    #[cfg(feature = "glue-node-set")]
    same_signature!(
        mkr_node_set_push,
        crate::glue::node_set::mkr_node_set_push,
        unsafe extern "C" fn(VALUE, *mut c_void)
    );

    #[cfg(feature = "bridge-string")]
    same_signature!(
        mkr_verify_text,
        crate::bridge::string::mkr_verify_text,
        unsafe extern "C" fn(VALUE, *const c_char)
    );
    #[cfg(feature = "bridge-string")]
    same_signature!(
        mkr_ruby_verified_text,
        crate::bridge::string::mkr_ruby_verified_text,
        unsafe extern "C" fn(VALUE, *const c_char) -> RubyText
    );

    #[cfg(feature = "glue-xml-node-read")]
    same_signature!(
        mkr_xml_node_unwrap,
        crate::glue::xml_node::mkr_xml_node_unwrap,
        unsafe extern "C" fn(VALUE) -> *mut c_void
    );
}
