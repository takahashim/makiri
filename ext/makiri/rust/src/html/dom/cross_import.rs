//! Cross-kind subtree translation for `Document#import_node`, for the arena
//! DOM: the Lexbor adapter's `cross_import`, step for step, over the arena's
//! handles. HTML -> XML reads the arena through the same typed API; XML ->
//! HTML builds through [`HtmlDocMut`], whose factories name each node as the
//! adapter's do.

use crate::falloc::{try_vec_with_capacity, OomOption, OomResult, VecPush};
use crate::html::dom::{
    has_ascii_uppercase, HtmlDocMut, HtmlElement, HtmlNode, NodeId as HtmlId, NsId, Place, RawNode,
};
use crate::xml::model::{ArenaKind, Document as XmlDoc, MutError, NodeId, Span};
use crate::xml::mutate;

/* ---- the node kinds on both sides ----
 *
 * `NodeType` is the crate-wide kind every layer reads; the XML arena arrives
 * as its own `ArenaKind`. The two now have distinct names, so a comparison
 * across representations reads for itself. */
use crate::node_type::NodeType;

/// A DOM name or value slice must fit `u32` - the mkr store's per-slice cap.
#[inline]
fn fits_u32(n: usize) -> Option<u32> {
    u32::try_from(n).ok()
}

/// The URI of an HTML node's namespace, borrowed from the SOURCE document's
/// interned table (stable for that document's lifetime), or `None` for the null
/// namespace - or for one too long for the XML store's `u32` slices.
fn html_ns_uri(n: HtmlNode<'_>) -> Option<&[u8]> {
    n.ns_uri().filter(|u| fits_u32(u.len()).is_some())
}

/* ---- the work stack, shared by both directions ---- */

/// One pending subtree: its source node and the copy its children go under.
struct Frame<S, D> {
    s: S,
    d: D,
}

/* ================= HTML (lxb) -> XML (mkr) ========================== */

/// Copy the source element's attributes onto the translated mkr element.
///
/// An attribute's namespace is its OWN ([`HtmlAttr::own_ns`], the reading
/// XPath and `Attr#namespace_uri` use): Lexbor stores a plain attribute under
/// its element's namespace, and reading that raw id put a parsed `q:y` inside
/// `<svg>` into SVG. A namespaced attribute is set WITH its namespace
/// (`set_attribute_ns`), so it keeps it wherever the copy is inserted. It used
/// to be a declaration of its prefix plus the bare name, and a declaration is
/// one attribute per prefix: an attribute `p:x` in `urn:other` on an element
/// `p:e` in `urn:p` redeclared `p` and moved the element into `urn:other`.
///
/// Two kinds of attribute do not cross as they stand:
/// * a declaration is left out - one parsed on a foreign element
///   (`xmlns:xlink`, in the XMLNS namespace) and one that is an ordinary HTML
///   attribute named `xmlns` alike. The translator declares each element's
///   namespace itself and gives each attribute its own, so a copied
///   declaration could only restate one or move one (`<div xmlns="urn:bogus">`
///   came out in `urn:bogus`, `<svg><g xmlns="urn:evil">` in `urn:evil`);
/// * one in NO namespace whose name XML cannot write as such - `fb:like`
///   (a prefix with no binding), `:href`, `@click` - crosses DOM-loose, named
///   as it is, as the DOM's clone has it: held, and refused by the
///   serializers. It was refused here, so a tree with Vue's or Alpine's
///   attributes could not be imported at all. `xml:` keeps its fixed meaning,
///   as the XML reader gives it.
fn h2x_copy_attrs(doc: &mut XmlDoc, s: HtmlElement<'_>, el: NodeId) -> Result<(), MutError> {
    for a in s.attrs() {
        let (name, value) = (a.qualified_name(), a.value());
        if fits_u32(name.len()).is_none() || fits_u32(value.len()).is_none() {
            return Err(MutError::Oom);
        }

        let own = a.own_ns();
        let decl = crate::xml::qname::xmlns_prefix(name);
        match (own, decl) {
            /* A declaration, parsed (a foreign element's `xmlns:xlink`) or an
             * HTML attribute that only looks like one: never copied. Every
             * name crosses with its namespace already - an element declares
             * its own, an attribute is given its - so a declaration can only
             * restate one, or move one: `<svg><g xmlns="urn:evil">` put `g`
             * and its children in `urn:evil`. */
            (Some(NsId::XMLNS), _) | (_, Some(_)) => {}
            /* An attribute in the XML namespace crosses WITH it: the DOM's
             * setAttributeNS gives it any prefix or none (`lang`, `p:lang`),
             * so its name alone does not say so, and the XML serializer
             * writes it as `xml:` whatever it is called. */
            (Some(NsId::XML), _) => {
                mutate::set_attribute_ns(doc, el, crate::xml::XML_NS_URI, name, value)?;
            }
            /* `xml:` keeps its fixed meaning, as the XML reader gives it:
             * an HTML `xml:lang` becomes the XML attribute. Any other name
             * crosses as the DOM's clone has it - in no namespace, named as
             * it is - which for `xlink:href`, `x-on:click`, `:href` or
             * `@click` is DOM-loose, as `set_loose_dom_attribute` makes it:
             * held, and refused by the serializers. */
            (None, _) if name.starts_with(b"xml:") => {
                mutate::set_attribute(doc, el, name, value)?;
            }
            (None, _) => {
                mutate::set_loose_dom_attribute(doc, el, name, value)?;
            }
            _ => {
                match a.own_ns_uri() {
                    Some(uri) => mutate::set_attribute_ns(doc, el, uri, name, value)?,
                    None => mutate::set_attribute(doc, el, name, value)?,
                };
            }
        }
    }
    Ok(())
}

/// Translate ONE Lexbor node into a fresh mkr node - its own fields and
/// attributes, NOT its children.
///
/// `None` to SKIP an unsupported type; an `Err` status fails the whole import.
/// The element namespace URIs one import has stored, so every element in
/// a namespace shares one copy of its URI - as the source shares one
/// Lexbor namespace id. A handful at most (HTML, SVG, MathML), so a list.
#[derive(Default)]
struct NsSpans<'a> {
    seen: Vec<(&'a [u8], Span)>,
}

impl<'a> NsSpans<'a> {
    /// The span of `uri` in `doc`, stored the first time it is asked for.
    fn span(&mut self, doc: &mut XmlDoc, uri: &'a [u8]) -> Result<Span, MutError> {
        if let Some(&(_, span)) = self.seen.iter().find(|(u, _)| *u == uri) {
            return Ok(span);
        }
        let span = mutate::stored_ns(doc, uri)?;
        /* A list that cannot grow only stops the sharing. */
        let _ = self.seen.falloc_push((uri, span));
        Ok(span)
    }
}

fn h2x_make<'a>(
    doc: &mut XmlDoc,
    s: HtmlNode<'a>,
    ns: &mut NsSpans<'a>,
) -> Result<Option<NodeId>, MutError> {
    let unchanged = |node| Ok(Some(node));
    let data = |n: HtmlNode<'a>| {
        let d = n.data().unwrap_or(&[]);
        fits_u32(d.len()).map(|_| d).or_oom::<MutError>()
    };

    if let Some(e) = s.element() {
        return h2x_element(doc, e, ns).map(Some);
    }
    let ty = match s.node_type() {
        NodeType::Text => ArenaKind::Text,
        NodeType::CDataSection => ArenaKind::CDataSection,
        NodeType::Comment => ArenaKind::Comment,
        NodeType::Pi => {
            let target = s.pi_target().unwrap_or(&[]);
            if fits_u32(target.len()).is_none() {
                return Err(MutError::Oom);
            }
            return unchanged(mutate::new_pi(doc, target, data(s)?)?);
        }
        NodeType::DocumentFragment => return unchanged(mutate::new_fragment(doc)?),
        /* An unsupported descendant type is skipped, not an error. */
        _ => return Ok(None),
    };
    unchanged(mutate::new_chardata(doc, ty, data(s)?)?)
}

/// [`h2x_make`] for an element: its name, in its namespace, and its
/// attributes.
///
/// No `xmlns` declaration is added: the DOM's importNode adds no attribute,
/// and the copy's namespace is its own from the start (`new_element_in`). It
/// used to carry one - visible as an attribute a browser's copy does not have,
/// which a DOM layer could not tell from one it was given - so insertion could
/// resolve the name, and `canonicalize` render it; the first is gone, and
/// `canonicalize` adds the declaration a name needs itself.
fn h2x_element<'a>(
    doc: &mut XmlDoc,
    e: HtmlElement<'a>,
    ns: &mut NsSpans<'a>,
) -> Result<NodeId, MutError> {
    let name = e.qualified_name();
    let Some(nl) = fits_u32(name.len()) else {
        return Err(MutError::Oom);
    };
    let euri = ns.span(doc, html_ns_uri(e.node()).unwrap_or(&[]))?;

    /* Three kinds of name. A PREFIXED one (an element that came from
     * XML) is made as written, prefix and namespace. An
     * unprefixed name with a colon (a parsed `fb:like`) is one DOM
     * local name, which XML cannot write as it stands: it is taken
     * VERBATIM as a DOM-loose name, so the copy is the DOM's element
     * and the XML serializer refuses it later - made strictly, `fb`
     * became a prefix bound to nothing, and the copy could not even be
     * inserted. Any other name is made strictly, and one that is a
     * valid DOM name but no XML QName is loose as well. A loose name
     * takes its namespace DIRECTLY (link-time resolution skips it). */
    let prefixed = e.node().has_prefix();
    let colon = name.iter().position(|&b| b == b':');
    let loose = |doc: &mut XmlDoc| {
        mutate::new_loose_dom_element_span(
            doc,
            name,
            crate::xml::qname::Split::unprefixed(nl),
            euri,
        )
    };
    let mut made = if colon.is_some() && !prefixed {
        loose(doc)
    } else {
        /* Its namespace decided now, as the DOM's clone has its own: made
         * undecided, it had none until inserted. */
        mutate::new_element_in_span(doc, name, euri)
    };
    if made.as_ref().err() == Some(&MutError::BadName) && !name.is_empty() {
        made = loose(doc);
    }
    let el = made?;
    h2x_copy_attrs(doc, e, el)?;
    Ok(el)
}

/// The first child to translate under `s`. A `<template>` gives its contents
/// first, then its own children ([`h2x_next`]): XML has no template contents,
/// so both become the copy's children, contents first - as XHTML writes them.
fn h2x_first_child(s: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    if s.is_html_template() {
        if let Some(c) = s.template_content().and_then(HtmlNode::first_child) {
            return Some(c);
        }
    }
    s.first_child()
}

/// The child to translate after `cur` under `s`: its next sibling, and after a
/// `<template>`'s last content node, the template's own first child. Its own
/// children (`appendChild` on the template) were dropped - only the contents
/// crossed, and the rest of the data vanished without a word.
fn h2x_next<'a>(s: HtmlNode<'a>, cur: HtmlNode<'a>) -> Option<HtmlNode<'a>> {
    if let Some(n) = cur.next() {
        return Some(n);
    }
    /* `cur` was in the contents fragment, not a child of the template. */
    if s.is_html_template() && cur.parent() != Some(s) {
        return s.first_child();
    }
    None
}

/// Deep- or shallow-copy an HTML subtree into the XML arena, detached. `src`
/// is a typed handle, so live for the call by its own contract.
pub fn cross_html_to_xml(
    xdoc: &mut XmlDoc,
    src: HtmlNode<'_>,
    deep: bool,
) -> Result<NodeId, MutError> {
    /* All or nothing, as the XML copies are: a copy that fails halfway -
     * the budget ran out - gives back what it took (`Document::rewind`,
     * sound here because nothing made below is linked into the tree or
     * handed out before the copy returns). */
    let mark = xdoc.mark();
    let copied = h2x_subtree(xdoc, src, deep);
    if copied.is_err() {
        xdoc.rewind(mark);
    }
    copied
}

/// [`cross_html_to_xml`]'s copy.
fn h2x_subtree(doc: &mut XmlDoc, src: HtmlNode<'_>, deep: bool) -> Result<NodeId, MutError> {
    let ns = &mut NsSpans::default();
    /* `None`: the root's type has no XML counterpart. */
    let root = h2x_make(doc, src, ns)?.ok_or(MutError::Type)?;

    if deep {
        let mut stack: Vec<Frame<HtmlNode<'_>, NodeId>> =
            try_vec_with_capacity(1).or_oom::<MutError>()?;
        stack
            .falloc_push(Frame { s: src, d: root })
            .or_oom::<MutError>()?;

        while let Some(f) = stack.pop() {
            let mut c = h2x_first_child(f.s);
            while let Some(child) = c {
                /* An error abandons the partial subtree. */
                if let Some(made) = h2x_make(doc, child, ns)? {
                    mutate::insert_child(doc, f.d, made)?;
                    if h2x_first_child(child).is_some() {
                        stack
                            .falloc_push(Frame { s: child, d: made })
                            .or_oom::<MutError>()?;
                    }
                }
                c = h2x_next(f.s, child);
            }
        }
    }

    Ok(root)
}

/* ================= XML -> HTML (arena) ========================== */

/// Copy the source element's attributes onto `el`, each appended as it is,
/// name and namespace - the DOM's clone step.
fn x2h_copy_attrs(
    d: &mut HtmlDocMut<'_>,
    doc: &XmlDoc,
    s: NodeId,
    el: HtmlId,
) -> Result<(), MutError> {
    let mut a = doc.first_attr(s);
    while let Some(attr) = a {
        let (val, qname, ns) = (doc.value(attr), doc.qname(attr), doc.ns(attr));
        d.append_attribute(el, (!ns.is_empty()).then_some(ns), qname, val)
            .map_err(|_| MutError::Oom)?;
        a = doc.next(attr);
    }
    Ok(())
}

/// Translate ONE XML node into a fresh, detached arena node; `None` to skip
/// an unsupported type. An XML CDATA section has no HTML counterpart, so that
/// one fails closed.
fn x2h_make(d: &mut HtmlDocMut<'_>, doc: &XmlDoc, s: NodeId) -> Result<Option<RawNode>, MutError> {
    let made = |n: Option<RawNode>| n.map(Some).or_oom::<MutError>();
    match doc.type_(s) {
        Some(ArenaKind::Element) => {
            let (prefix, ns) = (doc.prefix(s), doc.ns(s));
            let el = if prefix.is_empty()
                && !has_ascii_uppercase(doc.local(s))
                && d.read().lookup_ns(ns) == Some(NsId::HTML)
            {
                d.create_element(doc.qname(s)).or_oom::<MutError>()?
            } else {
                d.create_element_ns(doc.local(s), ns, prefix)
                    .or_oom::<MutError>()?
            };
            x2h_copy_attrs(d, doc, s, el.id())?;
            Ok(Some(el))
        }
        Some(ArenaKind::Text) => made(d.create_text(doc.value(s))),
        Some(ArenaKind::Comment) => made(d.create_comment(doc.value(s))),
        Some(ArenaKind::Pi) => made(d.create_pi(doc.local(s), doc.value(s))),
        Some(ArenaKind::CDataSection) => Err(MutError::Type),
        Some(ArenaKind::DocumentFragment) => made(d.create_fragment()),
        _ => Ok(None),
    }
}

/// Where a copied node's children go: a `<template>`'s contents, the node
/// itself otherwise.
fn link_target(d: &HtmlDocMut<'_>, n: RawNode) -> RawNode {
    match d.node(n).template_content() {
        Some(c) => RawNode::from(c),
        None => n,
    }
}

/// Deep- or shallow-copy an XML subtree into the arena document `d`,
/// detached.
pub fn cross_xml_to_html(
    d: &mut HtmlDocMut<'_>,
    doc: &XmlDoc,
    src: NodeId,
    deep: bool,
) -> Result<RawNode, MutError> {
    let root = x2h_make(d, doc, src)?.ok_or(MutError::Type)?;
    if deep {
        let mut stack: Vec<Frame<NodeId, RawNode>> =
            try_vec_with_capacity(1).or_oom::<MutError>()?;
        stack
            .falloc_push(Frame {
                s: src,
                d: link_target(d, root),
            })
            .or_oom::<MutError>()?;
        while let Some(f) = stack.pop() {
            let mut c = doc.first_child(f.s);
            while let Some(cid) = c {
                if let Some(dc) = x2h_make(d, doc, cid)? {
                    d.place(f.d, dc, Place::Child);
                    if doc.first_child(cid).is_some() {
                        stack
                            .falloc_push(Frame {
                                s: cid,
                                d: link_target(d, dc),
                            })
                            .or_oom::<MutError>()?;
                    }
                }
                c = doc.next(cid);
            }
        }
    }
    Ok(root)
}
