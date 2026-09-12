//! What the XML node glue reaches across to, and the small readers every one of
//! its modules needs.
//!
//! The node layout comes from `crate::xml::abi` - the XML engine's own
//! declaration - so nothing here restates a field offset or a type constant.

use core::ffi::{c_char, c_void};

use magnus::rb_sys::FromRawValue;
use magnus::{prelude::*, ExceptionClass, RString, Ruby, Value};
use rb_sys::VALUE;

pub use super::super::abi::{
    error_class, mkr_cDocument, mkr_cXmlDocument, mkr_cXmlDocumentFragment, mkr_doc_parsed,
    mkr_eError, mkr_eXmlSyntaxError, mkr_mXML, mkr_mXmlNodeMethods, mkr_node_set_new,
    mkr_node_set_push, mkr_parsed_xml_doc,
};
pub use crate::xml::abi::{
    Doc as XmlDoc, Node, T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE, T_DOCUMENT, T_ELEMENT,
    T_FRAGMENT, T_PI, T_TEXT,
};

/// `mkr_ruby_borrowed_text_t` / `_data_t` - a validated view anchored to the
/// Ruby String it came from. One layout, and the contract is the difference:
/// `text` has been checked for valid UTF-8 AND no NUL, `data` for UTF-8 only.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BorrowedText {
    pub value: VALUE,
    pub ptr: *const c_char,
    pub len: usize,
}

impl BorrowedText {
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

extern "C" {
    pub static mkr_cXmlNode: VALUE;
    pub static mkr_cXmlElement: VALUE;
    pub static mkr_cXmlAttr: VALUE;
    pub static mkr_cXmlText: VALUE;
    pub static mkr_cXmlComment: VALUE;
    pub static mkr_cXmlCDATASection: VALUE;
    pub static mkr_cXmlProcessingInstruction: VALUE;
    pub static mkr_cXmlDocumentType: VALUE;

    /// The XML node TypedData type, owned by `glue::node`.
    pub static mkr_xml_node_type: c_void;

    pub fn mkr_ruby_verified_text(input: VALUE, what: *const c_char) -> BorrowedText;

    /// The one byte-level xmlns detector, shared by the parser, the namespace
    /// resolver and this glue - so "is this an xmlns declaration" has a single
    /// answer. Reads the declared prefix ("" for the default xmlns) and the URI.
    pub fn mkr_xml_node_xmlns_decl(
        a: *const Node,
        prefix: *mut *const c_char,
        plen: *mut u32,
        uri: *mut *const c_char,
        ulen: *mut u32,
    ) -> c_int;
}

use core::ffi::c_int;

/// A node's field as a UTF-8 Ruby String. A NULL pointer is the empty string,
/// which is how the engine spells "no value".
///
/// # Safety
/// `ptr`/`len` must name `len` readable bytes, or be NULL.
pub unsafe fn str_field(ruby: &Ruby, ptr: *const c_char, len: u32) -> Value {
    if ptr.is_null() || len == 0 {
        return ruby.str_new("").as_value();
    }
    ruby.enc_str_new(
        core::slice::from_raw_parts(ptr as *const u8, len as usize),
        ruby.utf8_encoding(),
    )
    .as_value()
}

/// The same, but a NULL pointer means "absent" and becomes nil - the difference
/// between a DTD identifier that was omitted and one written as `""`.
///
/// # Safety
/// As [`str_field`].
pub unsafe fn str_field_or_nil(ruby: &Ruby, ptr: *const c_char, len: u32) -> Value {
    if ptr.is_null() {
        return ruby.qnil().as_value();
    }
    str_field(ruby, ptr, len)
}

/// A String from a Rust slice, tagged UTF-8.
pub fn utf8(ruby: &Ruby, bytes: &[u8]) -> RString {
    ruby.enc_str_new(bytes, ruby.utf8_encoding())
}

/// `Makiri::XML::SyntaxError`.
///
/// # Safety
/// After `Init_makiri`.
pub unsafe fn xml_syntax_error_class() -> ExceptionClass {
    ExceptionClass::from_value(Value::from_raw(mkr_eXmlSyntaxError))
        .expect("Makiri::XML::SyntaxError")
}

/// Is `v` an instance of the class in `klass`? The shared one, renamed for the
/// reading it gets here.
pub use super::super::abi::is_kind_of as is_a;
