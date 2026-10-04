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

mod bindings;
mod c14n;
mod out;
mod xml;

use crate::cbuf::Buf;
use crate::xml::model::{ArenaKind, Document as XmlDoc, NodeId};

/// Why serialization produced no output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A DOM-loose element name - created through the browser-DOM interop
    /// hatch - has no XML form.
    DomLooseName,
    /// The same for an attribute (`set_loose_dom_attribute`).
    DomLooseAttributeName,
    /// A namespace declaration holding a value Namespaces in XML forbids it
    /// to declare (`set_attribute_ns(XMLNS, "xmlns:p", "")`, which the DOM
    /// allows): its name is fine, and no XML can hold it.
    ForbiddenDeclaration,
    /// Character data XML cannot hold: a character outside XML 1.0's `Char`
    /// (`\f`, U+0001) in text, an attribute value, a comment, CDATA or a PI,
    /// or `--` in a comment, `?>` in a PI. The DOM holds all of it, so the
    /// mutators take it; it is refused where it would be written.
    UnwritableData,
    /// A DOM-loose DOCTYPE (`create_document_type`): a name or an id XML
    /// cannot write. Only [`to_xml`] refuses it; canonical form omits the
    /// document type declaration, so it has nothing to write wrong.
    DomLooseDoctype,
    /// An element in the XMLNS namespace (the DOM's `createElementNS(XMLNS,
    /// "xmlns")`): Namespaces in XML reserves it for declarations, so no
    /// element can be written in it.
    XmlnsElement,
    /// A PI target with a colon: the DOM creates one, but Namespaces in XML §7
    /// makes every PI target an NCName, and DOM Parsing's serializer refuses
    /// it too.
    PiTargetColon,
    /// A PI target that is `xml` in any case: the DOM creates one, XML 1.0
    /// §2.6 reserves the name, and DOM Parsing's serializer refuses it too.
    PiTargetReserved,
    /// The output exceeded its ceiling.
    OutputCap,
    /// The machine ran out of memory while serializing.
    Oom,
    /// The namespace scope table outgrew its index range - more bindings in
    /// scope at once than it can name.
    ScopeOverflow,
    /// The tree nests deeper than [`crate::xml::model::MAX_DEPTH`], the bound
    /// on the writers' recursion.
    TooDeep,
    /// Every prefix the writer can invent (`ns0` .. `ns99999`) is already bound
    /// in scope, so a namespace that needs a declaration cannot get one.
    PrefixSpace,
    /// Namespace planning exceeded its step budget - the document nests and
    /// re-declares prefixes deeply enough that resolving them all is not worth
    /// doing. Fails closed rather than running on.
    NamespaceBudget,
    /// Canonical XML renders the declarations the document holds, adding the
    /// declaration of a name's own prefix where one is missing - but it
    /// invents no prefix, so it refuses a tree where one prefix would have to
    /// mean two namespaces on one element (its own declaration says otherwise,
    /// or two of its names need it), or an unprefixed attribute has a
    /// namespace. Rendered anyway, the output named a different namespace.
    NamespaceMismatch,
    /// A name's prefix is bound to nothing - a detached element built with
    /// `q:e`, or an attribute whose prefix never resolved - so it has no
    /// well-formed form: `xmlns:q=""` is forbidden and a bare `q:` is unbound.
    UnboundPrefix,
}

impl crate::falloc::Oom for Failure {
    #[inline]
    fn oom() -> Self {
        Failure::Oom
    }
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
    if loose_doctype_under(doc, n) {
        return Err(Failure::DomLooseDoctype);
    }
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

/// Whether `n`'s output would hold a DOM-loose DOCTYPE: `n` is one, or the
/// Document holding one. Nothing else has a doctype below it.
fn loose_doctype_under(doc: &XmlDoc, n: NodeId) -> bool {
    let dt = match doc.type_(n) {
        Some(ArenaKind::DocumentType) => Some(n),
        Some(ArenaKind::Document) => doc
            .children(n)
            .find(|&c| doc.type_(c) == Some(ArenaKind::DocumentType)),
        _ => None,
    };
    dt.is_some_and(|dt| doc.is_loose_name(dt))
}

/// Why attribute `a` has no XML form, if it has none: a DOM-loose name, or a
/// declaration holding a value it may not declare.
fn unwritable_attr(doc: &XmlDoc, a: NodeId) -> Option<Failure> {
    if doc.is_loose_name(a) {
        Some(Failure::DomLooseAttributeName)
    } else if doc.forbidden_declaration(a) {
        Some(Failure::ForbiddenDeclaration)
    } else {
        None
    }
}

/// The first name under `root` that has no namespace-well-formed XML form.
fn unserializable_name(doc: &XmlDoc, root: NodeId) -> Option<Failure> {
    let mut cur = Some(root);
    let loose = |id| doc.is_loose_name(id);
    while let Some(id) = cur {
        match doc.type_(id) {
            Some(ArenaKind::Element) if loose(id) => return Some(Failure::DomLooseName),
            Some(ArenaKind::Element)
                if doc.span(doc.node(id).ns_uri) == crate::xml::XMLNS_NS_URI =>
            {
                return Some(Failure::XmlnsElement)
            }
            Some(ArenaKind::Element) => {
                if let Some(f) = doc.attributes(id).find_map(|a| unwritable_attr(doc, a)) {
                    return Some(f);
                }
            }
            /* An Attr serialized on its own is written by its name too. */
            Some(ArenaKind::Attribute) => {
                if let Some(f) = unwritable_attr(doc, id) {
                    return Some(f);
                }
            }
            Some(ArenaKind::Pi) if doc.span(doc.node(id).local).contains(&b':') => {
                return Some(Failure::PiTargetColon)
            }
            Some(ArenaKind::Pi)
                if crate::xml::chars::is_reserved_pi_target(doc.span(doc.node(id).local)) =>
            {
                return Some(Failure::PiTargetReserved)
            }
            _ => {}
        }
        cur = doc.preorder_next(root, id);
    }
    None
}
