//! Turning an XML node back into text: XML 1.0 and Inclusive Canonical XML 1.0.
//!
//! Ruby-free, like the rest of `xml`. The Ruby methods (`#to_xml`,
//! `#canonicalize`) live in `glue::xml_node::serialize`, which parses their
//! options, turns the bytes into a String and maps a [`Failure`] to its
//! exception.
//!
//! Output is always well-formed and re-parses to the same tree. xmlns
//! declarations ride along as ordinary attribute nodes, so namespaces
//! round-trip.
//!
//! The two forms share the output buffer and the escape table ([`out`]) and
//! nothing else: [`xml`] PLANS a prefix for every name, inventing one where the
//! natural prefix is taken, while [`c14n`] RENDERS the declarations the document
//! already holds. Keeping them apart is what stops one form's namespace rules
//! leaking into the other's.
//!
//! # Reading the arena
//!
//! The node is an index-arena `NodeId` and its bytes live in the document, so
//! these readers carry the document, and every name, value and prefix they hand
//! out borrows it for `'d`: the public entry points borrow the document for the
//! whole call.

#![forbid(unsafe_code)]

mod c14n;
mod out;
mod xml;

use crate::cbuf::Buf;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType};

/// Why serialization produced no output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A DOM-loose element name - created through the browser-DOM interop
    /// hatch - has no XML form.
    DomLooseName,
    /// A PI target with a colon: the DOM creates one, but Namespaces in XML §7
    /// makes every PI target an NCName, and DOM Parsing's serializer refuses
    /// it too.
    PiTargetColon,
    /// The output exceeded its ceiling, or memory ran out.
    Output,
    /// Namespace planning exceeded its step budget - the document nests and
    /// re-declares prefixes deeply enough that resolving them all is not worth
    /// doing. Fails closed rather than running on.
    NamespaceBudget,
}

/// The output ceiling for `doc`: generous, but proportional to its arena, so a
/// namespace-heavy tree cannot expand without bound.
pub fn output_cap(doc: &XmlDoc) -> usize {
    65536usize.saturating_add(doc.arena_bytes.saturating_mul(32))
}

/// `n` as XML 1.0, indented by `indent` spaces per level (0 for none).
///
/// The declaration names `encoding` when given, and otherwise UTF-8 when the
/// parsed document declared an encoding at all.
pub fn to_xml(
    doc: &XmlDoc,
    n: NodeId,
    indent: i32,
    encoding: Option<&[u8]>,
) -> Result<Buf, Failure> {
    write_with(doc, n, |b| xml::write(b, doc, n, indent, encoding))
}

/// `n` as Inclusive Canonical XML 1.0, with or without comments.
pub fn canonicalize(doc: &XmlDoc, n: NodeId, comments: bool) -> Result<Buf, Failure> {
    write_with(doc, n, |b| c14n::write(b, doc, n, comments))
}

/// The two forms share only this: refuse a name that has no XML spelling at all,
/// then hand a capped buffer to one writer. Which layout the output takes, and
/// how namespaces get there, is each writer's own business.
fn write_with(
    doc: &XmlDoc,
    n: NodeId,
    write: impl FnOnce(&mut Buf) -> Result<(), Failure>,
) -> Result<Buf, Failure> {
    if let Some(f) = unserializable_name(doc, n) {
        return Err(f);
    }
    let mut buf = Buf::new(output_cap(doc));
    write(&mut buf)?;
    Ok(buf)
}

/// The first name under `root` that has no namespace-well-formed XML form.
fn unserializable_name(doc: &XmlDoc, root: NodeId) -> Option<Failure> {
    let mut cur = Some(root);
    while let Some(id) = cur {
        match doc.type_(id) {
            Some(NodeType::Element)
                if doc.node(id).flags & crate::xml::FLAG_DOM_LOOSE_NAME != 0 =>
            {
                return Some(Failure::DomLooseName)
            }
            Some(NodeType::Pi) if doc.span(doc.node(id).local).contains(&b':') => {
                return Some(Failure::PiTargetColon)
            }
            _ => {}
        }
        cur = doc.preorder_next(root, id);
    }
    None
}
