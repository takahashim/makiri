//! What the XML node glue reaches across to, and the small readers every one of
//! its modules needs.
//!
//! The node layout comes from `crate::xml::model` - the XML engine's own
//! declaration - so nothing here restates a field offset or a type constant.

#![forbid(unsafe_code)]

use magnus::{prelude::*, ExceptionClass, RString, Ruby, Value};

pub use crate::bridge::lexbor::{doc_parsed, parsed_xml_doc};
pub use crate::bridge::ruby::error_class;
pub use crate::bridge::node_set::{node_set_new, node_set_with_fill};
pub use crate::xml::model::{Doc as XmlDoc, MutStatus, NodeId, NodeType, Span, Status};

/// The anchored Ruby-String view, from `glue::abi` - one definition for the
/// whole crate. Not `crate::text::BorrowedText`, which has no Ruby anchor.
pub use crate::bridge::string::{ruby_verified_text, RubyText};

pub use crate::bridge::lexbor::XML_NODE_TYPE;
pub use crate::init::CLASS_DOCUMENT;
pub use crate::init::CLASS_XML_ATTR;
pub use crate::init::CLASS_XML_CDATA_SECTION;
pub use crate::init::CLASS_XML_COMMENT;
pub use crate::init::CLASS_XML_DOCUMENT;
pub use crate::init::CLASS_XML_DOCUMENT_FRAGMENT;
pub use crate::init::CLASS_XML_DOCUMENT_TYPE;
pub use crate::init::CLASS_XML_ELEMENT;
pub use crate::init::CLASS_XML_NODE;
pub use crate::init::CLASS_XML_PROCESSING_INSTRUCTION;
pub use crate::init::CLASS_XML_TEXT;
pub use crate::init::EXC_ERROR;
pub use crate::init::EXC_XML_SYNTAX_ERROR;
pub use crate::init::MOD_XML;
pub use crate::init::MOD_XML_NODE_METHODS;

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
pub fn xml_syntax_error_class() -> ExceptionClass {
    ExceptionClass::from_value(EXC_XML_SYNTAX_ERROR.value()).expect("Makiri::XML::SyntaxError")
}

/// Is `v` an instance of the class in `klass`? The shared one, renamed for the
/// reading it gets here.
pub use crate::bridge::ruby::is_kind_of as is_a;
