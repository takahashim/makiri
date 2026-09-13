//! What the XML node glue reaches across to, and the small readers every one of
//! its modules needs.
//!
//! The node layout comes from `crate::xml::abi` - the XML engine's own
//! declaration - so nothing here restates a field offset or a type constant.

use core::ffi::c_char;

use magnus::rb_sys::FromRawValue;
use magnus::{prelude::*, ExceptionClass, RString, Ruby, Value};

pub use super::super::abi::{
    error_class, mkr_cDocument, mkr_cXmlDocument, mkr_cXmlDocumentFragment, mkr_doc_parsed,
    mkr_eError, mkr_eXmlSyntaxError, mkr_mXML, mkr_mXmlNodeMethods, mkr_node_set_new,
    mkr_node_set_push, mkr_parsed_xml_doc,
};
pub use crate::xml::abi::{
    Doc as XmlDoc, Node, T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE, T_DOCUMENT, T_ELEMENT,
    T_FRAGMENT, T_PI, T_TEXT,
};

/// The anchored Ruby-String view, from `glue::abi` - one definition for the
/// whole crate. Aliased rather than re-imported at every use site so the
/// existing `BorrowedText` spellings in this subtree keep working.
pub use crate::glue::abi::{mkr_ruby_verified_text, RubyText as BorrowedText};

pub use crate::glue::node::mkr_xml_node_type;
pub use crate::init::mkr_cXmlAttr;
pub use crate::init::mkr_cXmlCDATASection;
pub use crate::init::mkr_cXmlComment;
pub use crate::init::mkr_cXmlDocumentType;
pub use crate::init::mkr_cXmlElement;
pub use crate::init::mkr_cXmlNode;
pub use crate::init::mkr_cXmlProcessingInstruction;
pub use crate::init::mkr_cXmlText;
pub use crate::xml::ffi::mkr_xml_node_xmlns_decl;

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
