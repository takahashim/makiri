//! What the XML node glue reaches across to, and the small readers every one of
//! its modules needs.
//!
//! The node layout comes from `crate::xml::model` - the XML engine's own
//! declaration - so nothing here restates a field offset or a type constant.

use magnus::rb_sys::FromRawValue;
use magnus::{prelude::*, ExceptionClass, RString, Ruby, Value};

pub use super::super::abi::{
    cXmlDocument, doc_parsed, error_class, mkr_cDocument, mkr_cXmlDocumentFragment, mkr_eError,
    mkr_eXmlSyntaxError, mkr_mXML, mkr_mXmlNodeMethods, node_set_new, node_set_push,
    parsed_xml_doc,
};
pub use crate::xml::model::{Doc as XmlDoc, MutStatus, NodeId, NodeType, Span, Status};

/// The anchored Ruby-String view, from `glue::abi` - one definition for the
/// whole crate. Not `crate::text::BorrowedText`, which has no Ruby anchor.
pub use crate::glue::abi::{ruby_verified_text, RubyText};

pub use crate::glue::node::xml_node_type;
pub use crate::init::mkr_cXmlAttr;
pub use crate::init::mkr_cXmlCDATASection;
pub use crate::init::mkr_cXmlComment;
pub use crate::init::mkr_cXmlDocumentType;
pub use crate::init::mkr_cXmlElement;
pub use crate::init::mkr_cXmlNode;
pub use crate::init::mkr_cXmlProcessingInstruction;
pub use crate::init::mkr_cXmlText;

/// A field's bytes as a UTF-8 Ruby String (empty bytes -> `""`).
pub fn str_field(ruby: &Ruby, bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return ruby.str_new("").as_value();
    }
    ruby.enc_str_new(bytes, ruby.utf8_encoding()).as_value()
}

/// A document span as a Ruby String.
pub fn str_span(ruby: &Ruby, doc: &XmlDoc, s: Span) -> Value {
    str_field(ruby, doc.span(s))
}

/// As [`str_field`], but an ABSENT span becomes nil - the difference between a
/// DTD identifier that was omitted and one written as `""`.
pub fn str_span_or_nil(ruby: &Ruby, doc: &XmlDoc, s: Span) -> Value {
    if s.is_absent() {
        return ruby.qnil().as_value();
    }
    str_field(ruby, doc.span(s))
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
