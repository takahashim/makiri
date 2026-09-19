//! Arena bytes as Ruby Strings, for every module of the XML node glue.
//!
//! The document's bytes are valid UTF-8 - the reader validates on the way in,
//! and every mutator validates what it stores - so each String is tagged UTF-8.

#![forbid(unsafe_code)]

use magnus::{prelude::*, RString, Ruby, Value};

use crate::xml::model::{Doc as XmlDoc, Span};

/// A field's bytes as a UTF-8 Ruby String.
pub fn str_field(ruby: &Ruby, bytes: &[u8]) -> Value {
    utf8(ruby, bytes).as_value()
}

/// A document span as a Ruby String.
pub fn str_span(ruby: &Ruby, doc: &XmlDoc, s: Span) -> Value {
    str_field(ruby, doc.span(s))
}

/// A String from a Rust slice, tagged UTF-8.
pub fn utf8(ruby: &Ruby, bytes: &[u8]) -> RString {
    ruby.enc_str_new(bytes, ruby.utf8_encoding())
}
