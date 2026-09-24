//! Cross-kind subtree translation for `Document#import_node`.
//!
//! Makiri keeps HTML nodes (Lexbor's) and XML nodes (our index arena's) as
//! distinct representations that cannot share a tree. `import_node` bridges
//! them: a deep or shallow copy of a subtree from one representation into the
//! other, owned by the target document, returned DETACHED for the caller to
//! link.
//!
//! Ruby-free, and in `lexbor::adapter` rather than `glue` because it reads and
//! writes BOTH Lexbor and the XML document - exactly the bridge this layer is
//! for. The bridge entry points do the Ruby-side kind check, call one of these,
//! and wrap or raise.
//!
//! Both halves read through typed views: the XML side addresses a node by
//! [`NodeId`] in a [`XmlDoc`], the HTML side holds `html`'s handles, so no
//! Lexbor struct is read here. The only `unsafe` is where a [`RawNode`] or
//! [`RawDoc`] from the caller becomes a handle.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::lexbor::adapter::html::{
    BuildingElement, BuildingNode, HtmlDoc, HtmlElement, HtmlNode, RawDoc, RawNode, NS_HTML,
    NS_UNDEF, NS_XML, NS_XMLNS,
};
use crate::xml::model::{Document as XmlDoc, MutStatus, NodeId, NodeType};
use crate::xml::mutate;

/* ---- the node-type constants, generated on both sides ----
 *
 * `h::` is the HTML side, from the module that reads Lexbor's DOM; the mkr side
 * arrives as `NodeType` from the XML engine. Keeping the prefix is what makes a
 * comparison across representations read as one. */

mod h {
    pub use crate::lexbor::adapter::html::{
        TYPE_CDATA as CDATA, TYPE_COMMENT as COMMENT, TYPE_ELEMENT as ELEMENT,
        TYPE_FRAGMENT as FRAGMENT, TYPE_PI as PI, TYPE_TEXT as TEXT,
    };
}

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

/// One pending subtree, plus the default namespace in scope for its children.
///
/// `def` borrows the source document's interned namespace table, which is
/// [`html_ns_uri`]'s documented contract and outlives the walk. Carrying that
/// lifetime here rather than erasing it to `'static` is what keeps the
/// laundering in the one accessor that states the contract.
struct Frame<'a, S, D> {
    s: S,
    d: D,
    def: Option<&'a [u8]>,
}

/// Push, growing only when the stack is actually full.
#[inline]
fn push<'a, S, D>(stack: &mut Vec<Frame<'a, S, D>>, frame: Frame<'a, S, D>) -> Result<(), ()> {
    if stack.len() == stack.capacity() {
        let want = crate::falloc::grow_capacity(
            stack.capacity(),
            stack.len() + 1,
            core::mem::size_of::<Frame<'a, S, D>>(),
        )
        .ok_or(())?;
        stack.falloc_reserve_exact(want - stack.len())?;
    }
    stack.push(frame);
    Ok(())
}

/* ================= HTML (lxb) -> XML (mkr) ========================== */

/// Declare `xmlns` (no prefix) or `xmlns:PREFIX` = `uri` on the detached mkr
/// element, as an ordinary attribute.
fn declare_ns(doc: &mut XmlDoc, el: NodeId, prefix: &[u8], uri: &[u8]) -> MutStatus {
    if prefix.is_empty() {
        return status(mutate::set_attribute(doc, el, b"xmlns", uri));
    }
    let nlen = match 6usize.checked_add(prefix.len()).and_then(fits_u32) {
        Some(_) => 6 + prefix.len(),
        None => return MutStatus::Oom,
    };
    let mut name: Vec<u8> = match try_vec_with_capacity(nlen) {
        Some(v) => v,
        None => return MutStatus::Oom,
    };
    name.extend_from_slice(b"xmlns:");
    name.extend_from_slice(prefix);
    status(mutate::set_attribute(doc, el, &name, uri))
}

/// Collapse a safe mutator result onto the reported status.
#[inline]
fn status(r: Result<NodeId, MutStatus>) -> MutStatus {
    match r {
        Ok(_) => MutStatus::Ok,
        Err(st) => st,
    }
}

/// Copy the source element's attributes onto the translated mkr element,
/// declaring an `xmlns:PREFIX` for each foreign-prefixed one.
///
/// Two kinds of attribute are declarations-or-not depending on the side:
/// * one in the XMLNS namespace (a foreign element's `xmlns:xlink`) already IS
///   a declaration, so it is copied and nothing is declared for it - declaring
///   its "prefix" wrote `xmlns:xmlns="http://www.w3.org/2000/xmlns/"`, which
///   no parser accepts;
/// * one named `xmlns` / `xmlns:*` in NO namespace (on an HTML element) is an
///   ordinary attribute in HTML, but copied it would become a declaration and
///   move the element: `<div xmlns="urn:bogus">` came out in `urn:bogus`
///   instead of XHTML. The translator declares each element's real namespace
///   itself, so these are left out - the one attribute that cannot cross.
fn h2x_copy_attrs(doc: &mut XmlDoc, s: HtmlElement<'_>, el: NodeId) -> MutStatus {
    for a in s.attrs() {
        let (name, value) = (a.qualified_name(), a.value());
        if fits_u32(name.len()).is_none() || fits_u32(value.len()).is_none() {
            return MutStatus::Oom;
        }

        let ans = a.node().ns_id();
        if ans != NS_XMLNS && crate::xml::qname::xmlns_prefix(name).is_some() {
            continue;
        }
        if ans != NS_UNDEF && ans != NS_HTML && ans != NS_XML && ans != NS_XMLNS {
            if let Some(colon) = name.iter().position(|&b| b == b':') {
                if let Some(uri) = html_ns_uri(a.node()) {
                    let st = declare_ns(doc, el, &name[..colon], uri);
                    if st != MutStatus::Ok {
                        return st;
                    }
                }
            }
        }

        let st = status(mutate::set_attribute(doc, el, name, value));
        if st != MutStatus::Ok {
            return st;
        }
    }
    MutStatus::Ok
}

/// What [`h2x_make`] produced, plus the default namespace in scope for the new
/// node's children.
struct Made<'a> {
    node: NodeId,
    child_default: Option<&'a [u8]>,
}

/// Translate ONE Lexbor node into a fresh mkr node - its own fields and
/// attributes, NOT its children.
///
/// The invalid `NodeId` means SKIP an unsupported type; an `Err` status fails
/// the whole import.
fn h2x_make<'a>(
    doc: &mut XmlDoc,
    s: HtmlNode<'a>,
    parent_default: Option<&'a [u8]>,
) -> Result<Made<'a>, MutStatus> {
    let unchanged = |node| {
        Ok(Made {
            node,
            child_default: parent_default,
        })
    };
    let data = |n: HtmlNode<'a>| {
        let d = n.data().unwrap_or(&[]);
        fits_u32(d.len()).map(|_| d).ok_or(MutStatus::Oom)
    };

    match s.node_type() {
        h::ELEMENT => {
            let Some(e) = s.element() else {
                return unchanged(NodeId::INVALID);
            };
            let name = e.qualified_name();
            let Some(nl) = fits_u32(name.len()) else {
                return Err(MutStatus::Oom);
            };
            let euri = html_ns_uri(s);

            /* Strict first; a valid DOM element name that is not a well-formed
             * XML QName is taken VERBATIM as an unprefixed DOM-loose name. The
             * namespace is passed DIRECTLY (link-time resolution skips loose
             * names). */
            let mut made = mutate::new_element(doc, name);
            if made.as_ref().err() == Some(&MutStatus::BadName) && !name.is_empty() {
                made = mutate::new_loose_dom_element(
                    doc,
                    name,
                    crate::xml::qname::Split::unprefixed(nl),
                    euri.unwrap_or(&[]),
                );
            }
            let el = made?;

            let mut child_default = parent_default;
            if euri.unwrap_or(&[]) != parent_default.unwrap_or(&[]) {
                let st = declare_ns(doc, el, &[], euri.unwrap_or(&[]));
                if st != MutStatus::Ok {
                    return Err(st);
                }
                child_default = Some(euri.unwrap_or(&[]));
            }

            let st = h2x_copy_attrs(doc, e, el);
            if st != MutStatus::Ok {
                return Err(st);
            }
            Ok(Made {
                node: el,
                child_default,
            })
        }

        h::TEXT | h::CDATA | h::COMMENT => {
            let ty = match s.node_type() {
                h::TEXT => NodeType::Text,
                h::CDATA => NodeType::CData,
                _ => NodeType::Comment,
            };
            unchanged(mutate::new_chardata(doc, ty, data(s)?)?)
        }

        h::PI => {
            let target = s.pi_target().unwrap_or(&[]);
            if fits_u32(target.len()).is_none() {
                return Err(MutStatus::Oom);
            }
            unchanged(mutate::new_pi(doc, target, data(s)?)?)
        }

        h::FRAGMENT => unchanged(mutate::new_fragment(doc)?),

        /* An unsupported descendant type is skipped, not an error. */
        _ => unchanged(NodeId::INVALID),
    }
}

/// The first child to translate under `s`: a `<template>` descends into its
/// contents fragment, and has none when that fragment is missing.
fn h2x_first_child(s: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    if s.is_html_template() {
        return s.template_content()?.first_child();
    }
    s.first_child()
}

/// Deep- or shallow-copy an HTML subtree into the XML arena, detached.
///
/// # Safety
/// `src` is a live node whose document is not restructured during the call.
pub unsafe fn cross_html_to_xml(
    xdoc: &mut XmlDoc,
    src: RawNode,
    deep: bool,
) -> Result<NodeId, MutStatus> {
    let doc = xdoc;
    // SAFETY: the caller's contract.
    let src = unsafe { src.as_node() };

    let root = h2x_make(doc, src, None)?;
    if root.node.is_invalid() {
        return Err(MutStatus::Type); /* the root's type has no XML counterpart */
    }

    if deep {
        let mut stack: Vec<Frame<'_, HtmlNode<'_>, NodeId>> =
            try_vec_with_capacity(1).ok_or(MutStatus::Oom)?;
        push(
            &mut stack,
            Frame {
                s: src,
                d: root.node,
                def: root.child_default,
            },
        )
        .map_err(|_| MutStatus::Oom)?;

        while let Some(f) = stack.pop() {
            let mut c = h2x_first_child(f.s);
            while let Some(child) = c {
                /* An error abandons the partial subtree. */
                let made = h2x_make(doc, child, f.def)?;
                if !made.node.is_invalid() {
                    let st = mutate::insert_child(doc, f.d, made.node);
                    if st != MutStatus::Ok {
                        return Err(st);
                    }
                    if h2x_first_child(child).is_some() {
                        push(
                            &mut stack,
                            Frame {
                                s: child,
                                d: made.node,
                                def: made.child_default,
                            },
                        )
                        .map_err(|_| MutStatus::Oom)?;
                    }
                }
                c = child.next();
            }
        }
    }

    Ok(root.node)
}

/* ================= XML (mkr) -> HTML (lxb) ========================== */

/// Copy the source element's attributes onto the element being translated into.
///
/// The document comes from `el` itself, so there is no second handle to keep in
/// step with it.
fn x2h_copy_attrs(doc: &XmlDoc, s: NodeId, el: BuildingElement<'_>) -> MutStatus {
    let mut a = doc.attrs(s);
    while let Some(attr) = a {
        let (val, qname, ns) = (doc.value(attr), doc.qname(attr), doc.ns(attr));
        let stored = if ns.is_empty() {
            el.set_attribute(qname, val)
        } else {
            el.append_ns_attribute(ns, qname, val)
        };
        if !stored {
            return MutStatus::Oom;
        }
        a = doc.next(attr);
    }
    MutStatus::Ok
}

/// Translate ONE mkr node into a fresh, detached Lexbor node.
///
/// `None` to SKIP an unsupported type - an ordinary outcome, which is why it is
/// not an `Err`. An XML CDATA section has no HTML counterpart, so that one fails
/// closed rather than degrading to a text node.
fn x2h_make<'doc>(
    hdoc: HtmlDoc<'doc>,
    doc: &XmlDoc,
    s: NodeId,
) -> Result<Option<BuildingNode<'doc>>, MutStatus> {
    let made = |n: Option<BuildingNode<'doc>>| n.map(Some).ok_or(MutStatus::Oom);

    match doc.type_(s) {
        Some(NodeType::Element) => {
            let el = hdoc.create_element(doc.qname(s)).ok_or(MutStatus::Oom)?;
            el.set_ns(hdoc.intern_ns(doc.ns(s)));

            let st = x2h_copy_attrs(doc, s, el);
            if st != MutStatus::Ok {
                return Err(st);
            }
            Ok(Some(el.as_node()))
        }
        Some(NodeType::Text) => made(hdoc.create_text(doc.value(s))),
        Some(NodeType::Comment) => made(hdoc.create_comment(doc.value(s))),
        Some(NodeType::Pi) => made(hdoc.create_pi(doc.local(s), doc.value(s))),
        Some(NodeType::CData) => Err(MutStatus::Type), /* HTML has no CDATA section */
        Some(NodeType::Fragment) => made(hdoc.create_fragment()),
        _ => Ok(None), /* unsupported descendant type: skip */
    }
}

/// Deep- or shallow-copy an XML subtree into the Lexbor arena, detached.
///
/// # Safety
/// `hdoc` is a live document, not restructured during the call.
pub unsafe fn cross_xml_to_html(
    hdoc: RawDoc,
    doc: &XmlDoc,
    src: NodeId,
    deep: bool,
) -> Result<RawNode, MutStatus> {
    // SAFETY: the caller's contract.
    let hdoc = unsafe { hdoc.as_doc() };

    /* `None` is a root whose type has no HTML counterpart. */
    let root = x2h_make(hdoc, doc, src)?.ok_or(MutStatus::Type)?;

    if deep {
        let mut stack: Vec<Frame<NodeId, BuildingNode<'_>>> =
            try_vec_with_capacity(1).ok_or(MutStatus::Oom)?;
        push(
            &mut stack,
            Frame {
                s: src,
                d: root.link_target(),
                def: None,
            },
        )
        .map_err(|_| MutStatus::Oom)?;

        while let Some(f) = stack.pop() {
            let mut c = doc.first_child(f.s);
            while let Some(cid) = c {
                /* An error abandons the partial subtree in mraw. */
                if let Some(dc) = x2h_make(hdoc, doc, cid)? {
                    f.d.insert_child(dc);
                    if doc.first_child(cid).is_some() {
                        push(
                            &mut stack,
                            Frame {
                                s: cid,
                                d: dc.link_target(),
                                def: None,
                            },
                        )
                        .map_err(|_| MutStatus::Oom)?;
                    }
                }
                c = doc.next(cid);
            }
        }
    }

    Ok(RawNode::from(root))
}
