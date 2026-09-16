//! Cross-kind subtree translation for `Document#import_node`
//! (dom_adapter/cross_import.c).
//!
//! Makiri keeps HTML nodes (Lexbor `lxb_dom_node_t`) and XML nodes
//! (index-arena `mkr_xml_node_t`) as distinct representations that cannot share
//! a tree. `import_node` bridges them: a deep or shallow copy of a subtree from
//! one representation into the other, owned by the target document, returned
//! DETACHED for the caller to link.
//!
//! Ruby-free, and in `dom_adapter` rather than `glue` because it reads and
//! writes BOTH Lexbor and the XML document - exactly the bridge this layer is
//! for. The glue entry points do the Ruby-side kind check, call one of these,
//! and wrap or raise.
//!
//! An XML node is addressed by [`NodeId`] and its bytes/links live in the
//! [`Document`], so the XML half of this module threads a `&Document`; the
//! Lexbor half keeps raw pointers.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::lexbor::adapter::html::{
    BuildingElement, BuildingNode, HtmlDoc, NS_HTML, NS_UNDEF, NS_XML, TAG_TEMPLATE,
};
use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::lexbor_abi::{self as lxb, LxbDoc, LxbElement, LxbNode};
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

/* Every Lexbor entry point below comes from the generated bindings. */
use lxb::{
    lxb_dom_document_create_comment, lxb_dom_document_create_document_fragment,
    lxb_dom_document_create_element, lxb_dom_document_create_processing_instruction,
    lxb_dom_document_create_text_node, lxb_ns_by_id,
};

/// A DOM name or value slice must fit `u32` - the mkr store's per-slice cap.
#[inline]
fn fits_u32(n: usize) -> Option<u32> {
    u32::try_from(n).ok()
}

/// The URI for an HTML node's namespace id, borrowed from the SOURCE document's
/// interned table (stable for that document's lifetime), or `None` for the null
/// namespace.
unsafe fn html_ns_uri<'a>(n: *const LxbNode) -> Option<&'a [u8]> {
    if (*n).ns == NS_UNDEF {
        return None;
    }
    let doc = (*n).owner_document;
    if doc.is_null() || (*doc).ns.is_null() {
        return None;
    }
    let mut len = 0usize;
    let u = lxb_ns_by_id((*doc).ns, (*n).ns, &mut len);
    if u.is_null() || fits_u32(len).is_none() {
        return None;
    }
    Some(core::slice::from_raw_parts(u, len))
}

/* ---- the work stack, shared by both directions ---- */

struct Frame<S, D> {
    s: S,
    d: D,
    def: Option<&'static [u8]>,
}

/// Push, growing only when the stack is actually full.
#[inline]
fn push<S, D>(stack: &mut Vec<Frame<S, D>>, frame: Frame<S, D>) -> Result<(), ()> {
    if stack.len() == stack.capacity() {
        let want = crate::falloc::grow_capacity(
            stack.capacity(),
            stack.len() + 1,
            core::mem::size_of::<Frame<S, D>>(),
        )
        .ok_or(())?;
        stack.mkr_reserve_exact(want - stack.len())?;
    }
    stack.push(frame);
    Ok(())
}

/// A `<template>`'s content fragment, when `n` is an HTML `<template>`.
unsafe fn template_content(n: *const LxbNode) -> Option<*mut c_void> {
    if (*n).type_ == h::ELEMENT && (*n).local_name == TAG_TEMPLATE && (*n).ns == NS_HTML {
        return Some((*(n as *const lxb::lxb_html_template_element_t)).content as *mut c_void);
    }
    None
}

/* ================= HTML (lxb) -> XML (mkr) ========================== */

/// Declare `xmlns` (no prefix) or `xmlns:PREFIX` = `uri` on the detached mkr
/// element, as an ordinary attribute.
unsafe fn declare_ns(doc: &mut XmlDoc, el: NodeId, prefix: &[u8], uri: &[u8]) -> MutStatus {
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
unsafe fn status(r: Result<NodeId, MutStatus>) -> MutStatus {
    match r {
        Ok(_) => MutStatus::Ok,
        Err(st) => st,
    }
}

/// Copy the source element's attributes onto the translated mkr element,
/// declaring an `xmlns:PREFIX` for each foreign-prefixed one.
unsafe fn h2x_copy_attrs(doc: &mut XmlDoc, s: *mut LxbNode, el: NodeId) -> MutStatus {
    let mut a = lxb::lxb_dom_element_first_attribute_noi(s as *mut LxbElement);
    while !a.is_null() {
        let mut anl = 0usize;
        let mut avl = 0usize;
        let an = lxb::lxb_dom_attr_qualified_name(a, &mut anl);
        let av = lxb::lxb_dom_attr_value_noi(a, &mut avl);
        if fits_u32(anl).is_none() || fits_u32(avl).is_none() {
            return MutStatus::Oom;
        }
        let name = core::slice::from_raw_parts(an, anl);
        let value = if av.is_null() {
            &[][..]
        } else {
            core::slice::from_raw_parts(av, avl)
        };

        let ans = (*a).node.ns;
        if ans != NS_UNDEF && ans != NS_HTML && ans != NS_XML {
            if let Some(colon) = name.iter().position(|&b| b == b':') {
                if let Some(uri) = html_ns_uri(&(*a).node) {
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
        a = lxb::lxb_dom_element_next_attribute_noi(a);
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
unsafe fn h2x_make<'a>(
    doc: &mut XmlDoc,
    s: *mut LxbNode,
    parent_default: Option<&'a [u8]>,
) -> Result<Made<'a>, MutStatus> {
    let unchanged = |node| {
        Ok(Made {
            node,
            child_default: parent_default,
        })
    };

    match (*s).type_ {
        h::ELEMENT => {
            let mut nl = 0usize;
            let nm = lxb::lxb_dom_element_qualified_name(s as *mut LxbElement, &mut nl);
            if fits_u32(nl).is_none() {
                return Err(MutStatus::Oom);
            }
            let name = core::slice::from_raw_parts(nm, nl);
            let euri = html_ns_uri(s);

            /* Strict first; a valid DOM element name that is not a well-formed
             * XML QName is taken VERBATIM as an unprefixed DOM-loose name. The
             * namespace is passed DIRECTLY (link-time resolution skips loose
             * names). */
            let mut made = mutate::new_element(doc, name);
            if made.as_ref().err() == Some(&MutStatus::BadName) && !name.is_empty() {
                made =
                    mutate::new_loose_dom_element(doc, name, 0, 0, nl as u32, euri.unwrap_or(&[]));
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

            let st = h2x_copy_attrs(doc, s, el);
            if st != MutStatus::Ok {
                return Err(st);
            }
            Ok(Made {
                node: el,
                child_default,
            })
        }

        h::TEXT | h::CDATA | h::COMMENT => {
            let d = &(*(s as *const lxb::lxb_dom_character_data_t)).data;
            let len = match fits_u32(d.length) {
                Some(l) => l,
                None => return Err(MutStatus::Oom),
            };
            let ty = match (*s).type_ {
                h::TEXT => NodeType::Text,
                h::CDATA => NodeType::CData,
                _ => NodeType::Comment,
            };
            let text = if d.data.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(d.data, len as usize)
            };
            unchanged(mutate::new_chardata(doc, ty, text)?)
        }

        h::PI => {
            let mut tl = 0usize;
            let tg = lxb::lxb_dom_processing_instruction_target_noi(s as *mut _, &mut tl);
            let d = &(*(s as *const lxb::lxb_dom_character_data_t)).data;
            if fits_u32(tl).is_none() || fits_u32(d.length).is_none() {
                return Err(MutStatus::Oom);
            }
            let target = core::slice::from_raw_parts(tg, tl);
            let data = if d.data.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(d.data, d.length)
            };
            unchanged(mutate::new_pi(doc, target, data)?)
        }

        h::FRAGMENT => {
            let f = doc
                .new_node(NodeType::Fragment)
                .map_err(|_| MutStatus::Oom)?;
            unchanged(f)
        }

        /* An unsupported descendant type is skipped, not an error. */
        _ => unchanged(NodeId::INVALID),
    }
}

/// The children to translate under `s` (a `<template>` descends into content).
unsafe fn h2x_children_of(s: *mut LxbNode) -> *mut LxbNode {
    match template_content(s) {
        Some(content) if !content.is_null() => (*(content as *mut LxbNode)).first_child,
        Some(_) => core::ptr::null_mut(),
        None => (*s).first_child,
    }
}

/// Deep- or shallow-copy an HTML subtree into the XML arena, detached.
pub unsafe fn cross_html_to_xml(
    xdoc: *mut XmlDoc,
    src: *mut LxbNode,
    deep: bool,
    out: *mut NodeId,
) -> MutStatus {
    *out = NodeId::INVALID;
    let doc = &mut *xdoc;

    let root = match h2x_make(doc, src, None) {
        Ok(m) => m,
        Err(st) => return st,
    };
    if root.node.is_invalid() {
        return MutStatus::Type; /* the root's type has no XML counterpart */
    }

    if deep {
        let mut stack: Vec<Frame<*mut LxbNode, NodeId>> = match try_vec_with_capacity(1) {
            Some(v) => v,
            None => return MutStatus::Oom,
        };
        let rdef: Option<&'static [u8]> = core::mem::transmute(root.child_default);
        if push(
            &mut stack,
            Frame {
                s: src,
                d: root.node,
                def: rdef,
            },
        )
        .is_err()
        {
            return MutStatus::Oom;
        }

        while let Some(f) = stack.pop() {
            let mut c = h2x_children_of(f.s);
            while !c.is_null() {
                let made = match h2x_make(doc, c, f.def) {
                    Ok(m) => m,
                    Err(st) => return st, /* partial subtree abandoned */
                };
                if !made.node.is_invalid() {
                    let st = mutate::insert_child(doc, f.d, made.node);
                    if st != MutStatus::Ok {
                        return st;
                    }
                    if !h2x_children_of(c).is_null() {
                        let cdef: Option<&'static [u8]> = core::mem::transmute(made.child_default);
                        if push(
                            &mut stack,
                            Frame {
                                s: c,
                                d: made.node,
                                def: cdef,
                            },
                        )
                        .is_err()
                        {
                            return MutStatus::Oom;
                        }
                    }
                }
                c = (*c).next;
            }
        }
    }

    *out = root.node;
    MutStatus::Ok
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
    let value = || doc.value(s);
    /* Every `create_*` below returns a fresh, detached node of `hdoc` - which
     * outlives 'doc - or null when the arena could not take it.
     * SAFETY: that is exactly this type's contract, and `from_raw` turns the
     * null into the Oom. */
    let made = |n: *mut LxbNode| unsafe { BuildingNode::from_raw(n) }.ok_or(MutStatus::Oom);

    match doc.type_(s) {
        Some(NodeType::Element) => {
            let qname = doc.qname(s);
            /* SAFETY: a live document, and Lexbor copies the name. */
            let el = unsafe {
                lxb_dom_document_create_element(
                    hdoc.as_raw(),
                    qname.as_ptr(),
                    qname.len(),
                    core::ptr::null_mut(),
                )
            };
            /* SAFETY: as `made` - a fresh, detached element, or null. */
            let Some(el) = (unsafe { BuildingElement::from_raw(el) }) else {
                return Err(MutStatus::Oom);
            };
            el.set_ns(hdoc.intern_ns(doc.ns(s)));

            let st = x2h_copy_attrs(doc, s, el);
            if st != MutStatus::Ok {
                return Err(st);
            }
            Ok(Some(el.as_node()))
        }

        Some(NodeType::Text) => {
            let v = value();
            /* SAFETY: see `made`. */
            let t =
                unsafe { lxb_dom_document_create_text_node(hdoc.as_raw(), v.as_ptr(), v.len()) };
            made(t as *mut LxbNode).map(Some)
        }

        Some(NodeType::Comment) => {
            let v = value();
            /* SAFETY: see `made`. */
            let c = unsafe { lxb_dom_document_create_comment(hdoc.as_raw(), v.as_ptr(), v.len()) };
            made(c as *mut LxbNode).map(Some)
        }

        Some(NodeType::Pi) => {
            let target = doc.local(s);
            let v = value();
            /* SAFETY: see `made`. */
            let pi = unsafe {
                lxb_dom_document_create_processing_instruction(
                    hdoc.as_raw(),
                    target.as_ptr(),
                    target.len(),
                    v.as_ptr(),
                    v.len(),
                )
            };
            made(pi as *mut LxbNode).map(Some)
        }

        Some(NodeType::CData) => Err(MutStatus::Type), /* HTML has no CDATA section */

        Some(NodeType::Fragment) => {
            /* SAFETY: see `made`. */
            let f = unsafe { lxb_dom_document_create_document_fragment(hdoc.as_raw()) };
            made(f as *mut LxbNode).map(Some)
        }

        _ => Ok(None), /* unsupported descendant type: skip */
    }
}

/// Deep- or shallow-copy an XML subtree into the Lexbor arena, detached.
pub unsafe fn cross_xml_to_html(
    hdoc: *mut LxbDoc,
    xdoc: *const XmlDoc,
    src: NodeId,
    deep: bool,
    out: *mut *mut LxbNode,
) -> MutStatus {
    *out = core::ptr::null_mut();
    let Some(hdoc) = HtmlDoc::from_raw(hdoc) else {
        return MutStatus::Internal; /* a null destination document */
    };
    let doc = &*xdoc;

    let root = match x2h_make(hdoc, doc, src) {
        Ok(Some(n)) => n,
        Ok(None) => return MutStatus::Type, /* the root's type has no HTML counterpart */
        Err(st) => return st,
    };

    if deep {
        let mut stack: Vec<Frame<NodeId, BuildingNode<'_>>> = match try_vec_with_capacity(1) {
            Some(v) => v,
            None => return MutStatus::Oom,
        };
        if push(
            &mut stack,
            Frame {
                s: src,
                d: root.link_target(),
                def: None,
            },
        )
        .is_err()
        {
            return MutStatus::Oom;
        }

        while let Some(f) = stack.pop() {
            let mut c = doc.first_child(f.s);
            while let Some(cid) = c {
                let dc = match x2h_make(hdoc, doc, cid) {
                    Ok(n) => n,
                    Err(st) => return st, /* partial subtree abandoned in mraw */
                };
                if let Some(dc) = dc {
                    f.d.insert_child(dc);
                    if doc.first_child(cid).is_some()
                        && push(
                            &mut stack,
                            Frame {
                                s: cid,
                                d: dc.link_target(),
                                def: None,
                            },
                        )
                        .is_err()
                    {
                        return MutStatus::Oom;
                    }
                }
                c = doc.next(cid);
            }
        }
    }

    *out = root.as_raw();
    MutStatus::Ok
}
