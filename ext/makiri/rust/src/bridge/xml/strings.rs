//! A String argument checked for an XML node: the text contract, and short
//! enough for an arena span.

#![forbid(unsafe_code)]

use super::*;

/// A String argument verified as an engine string - valid UTF-8, no NUL - and
/// short enough for an arena span (4 GiB).
pub fn verified_text(ruby: &Ruby, v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(ruby_verified_text(ruby, v, what)?)
}

/// `t`, unless it is too long for an XML node's `u32` span.
fn fits_xml_node<T: core::ops::Deref<Target = str>>(t: T) -> Result<T, Error> {
    if u32::try_from(t.len()).is_err() {
        return Err(makiri_error("string too long for an XML node (max 4 GiB)"));
    }
    Ok(t)
}

/// [`verified_text`] for a name given to a factory or a setter: a NUL raises
/// `ArgumentError`, as any other refused name does
/// ([`crate::bridge::string::ruby_verified_name`]).
pub fn verified_name(ruby: &Ruby, v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_name(ruby, v, what)?)
}

/// [`verified_text`] for DATA - text, comment, CDATA and PI content, an
/// attribute value: valid UTF-8, and NUL allowed, as the DOM allows it there
/// ([`crate::bridge::string::ruby_verified_data`], what HTML's data takes).
/// XML cannot write U+0000, so the serializers refuse a tree holding one, as
/// they refuse any character XML has no `Char` for.
pub fn verified_data(
    ruby: &Ruby,
    v: Value,
    what: &str,
) -> Result<crate::bridge::string::RubyData, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_data(ruby, v, what)?)
}
