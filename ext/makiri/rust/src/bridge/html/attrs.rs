//! The attribute and data edits, the checked views passed straight to the
//! arena.

#![forbid(unsafe_code)]

use super::*;
use crate::xml::dom_name::{Name, QName};

/* ------------------------------------------------------------------ *
 * verified strings into the arena                                    *
 * ------------------------------------------------------------------ *
 * The node methods live in `glue::html_node::mutate`. A checked view reads as
 * a plain `&str` (`RubyStr`'s `Deref`, over its own copy of the bytes), and each
 * primitive passes it straight to an arena edit that copies what it keeps. A
 * primitive answers what the edit answered; the method words the error. */

/// `el[name] = value` on the element `el`; `Err` when it could not be stored.
pub fn set_attribute(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    name: Name<'_>,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_attribute(el, name, value.as_bytes())
}

/// Set the attribute `qname` in namespace `ns` (`None` = none), matching an
/// existing one on (namespace, local name) - the DOM key - rather than on the
/// qualified name. `Err` when it could not be stored.
///
/// `ns` comes from `string::namespace_arg`, so an empty URI already reads as
/// `None`.
pub fn set_attribute_ns(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    ns: Option<&[u8]>,
    qname: QName<'_>,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_attribute_ns(el, ns, qname, value.as_bytes())
}

/// Remove the attribute `local` in namespace `ns` (`None` = none) from the
/// element `el`; whether there was one. `ns` comes from
/// `string::namespace_arg`.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "the attribute found, removed in the same edit"
    )
)]
pub fn remove_attribute_ns(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    ns: Option<&[u8]>,
    local: &RubyText,
) -> bool {
    /* Looked up, not interned: a namespace the document never interned is
     * one no attribute here carries, so there is nothing to remove - and a
     * lookup can neither fail nor grow the table. */
    let want_ns = match ns {
        Some(uri) => match d.read().lookup_ns(uri) {
            Some(id) => Some(id),
            None => return false,
        },
        None => None,
    };
    let found = d
        .node(el)
        .element()
        .and_then(|e| e.attr_by_ns(want_ns, local.as_bytes()))
        .map(|a| RawNode::from(a.node()));
    match found {
        Some(attr) => {
            d.attr_remove(el, attr);
            true
        }
        None => false,
    }
}

/// `el.delete(name)` on the element `el`.
pub fn remove_attribute(d: &mut HtmlDocMut<'_>, el: RawNode, name: &RubyText) {
    d.remove_attribute(el, name.as_bytes());
}

/// `node.content = text`; `Err` when it could not be stored.
pub fn set_text_content(
    d: &mut HtmlDocMut<'_>,
    node: RawNode,
    text: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_text_content(node, text.as_bytes())
}

/// Whether `name` is one the DOM accepts for a doctype.
pub fn valid_doctype_name(name: &RubyText) -> bool {
    HtmlDoc::valid_doctype_name(name.as_bytes())
}
