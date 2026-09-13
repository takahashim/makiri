//! Cross-kind subtree translation for `Document#import_node`
//! (dom_adapter/cross_import.c).
//!
//! Makiri keeps HTML nodes (Lexbor `lxb_dom_node_t`) and XML nodes
//! (`mkr_xml_node_t`) as distinct representations that cannot share a tree.
//! `import_node` bridges them: a deep or shallow copy of a subtree from one
//! representation into the other, owned by the target document, returned
//! DETACHED for the caller to link.
//!
//! Ruby-free, and in `dom_adapter` rather than `glue` because it reads and
//! writes BOTH Lexbor and the XML arena - exactly the bridge this layer is for.
//! The glue entry points do the Ruby-side kind check, call one of these, and
//! wrap or raise.
//!
//! Both directions share three properties:
//!
//! - the destination subtree is built DETACHED and only then returned, never
//!   linked into a live tree mid-build, so a failure abandons a self-contained
//!   partial subtree in the destination arena - freed with the document, the
//!   same fail-closed model the XML deep-copy uses;
//! - the source is walked with an explicit heap stack, never C recursion, so a
//!   deep tree cannot exhaust the stack;
//! - failure is reported as an `mkr_xml_mut_status_t`, which the Ruby entry maps.
//!
//! # Namespaces
//!
//! **HTML -> XML.** An mkr node's namespace is resolved from `xmlns`
//! declarations at INSERTION time, so a directly-set `ns_uri` would be
//! overwritten when the imported subtree is later linked. Declarations are
//! therefore synthesized: each element declares `xmlns="URI"` when its namespace
//! differs from the one inherited from its translated parent, and a
//! foreign-prefixed attribute (`xlink:*`) gets an `xmlns:PREFIX` on its element.
//! The predefined `xml:` prefix needs none.
//!
//! **XML -> HTML.** Lexbor stores a namespace as an interned id, so the
//! element's `node.ns` is set from the URI (interning any URI through
//! `lxb_ns_append`) and a namespaced attribute is built with
//! `lxb_dom_attr_set_name_ns`.
//!
//! # Why this one feature implies the XML reader
//!
//! It calls the XML arena's factories directly rather than through their C ABI.
//! Declaring them here as `extern "C"` would give each symbol a second Rust
//! declaration whenever the `xml` feature is also on - the "redeclared with a
//! different signature" class that has now cost this port several rounds. The
//! feature depends on `xml` instead, so there is one definition and no
//! declaration at all.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::lexbor_abi::{self as lxb, LxbDoc, LxbElement, LxbNode};
use crate::xml::abi::{
    Doc as XmlDoc, Node as XmlNode, QName, MUT_BAD_NAME, MUT_OK, MUT_OOM, MUT_TYPE,
};
use crate::xml::mutate;

/* ---- the node-type constants, generated on both sides ---- */

mod h {
    use crate::lexbor_abi as lxb;
    pub const ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
    pub const TEXT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT;
    pub const CDATA: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION;
    pub const COMMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT;
    pub const PI: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION;
    pub const FRAGMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
}

const NS_UNDEF: usize = lxb::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
const NS_XML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_XML as usize;
const TAG_TEMPLATE: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_TEMPLATE as usize;

use crate::xml::abi::{
    T_CDATA as X_CDATA, T_COMMENT as X_COMMENT, T_ELEMENT as X_ELEMENT, T_FRAGMENT as X_FRAGMENT,
    T_PI as X_PI, T_TEXT as X_TEXT,
};

/* Every Lexbor entry point below comes from the generated bindings. They were
 * hand-declared here first, and rustc reported six of them "redeclared with a
 * different signature" against the generated ones - the same class the rest of
 * this port keeps running into, and the reason build.rs allowlists them. */
use lxb::{
    lxb_dom_attr_interface_create, lxb_dom_attr_set_value, lxb_dom_document_create_comment,
    lxb_dom_document_create_document_fragment, lxb_dom_document_create_element,
    lxb_dom_document_create_processing_instruction, lxb_dom_document_create_text_node,
    lxb_dom_element_attr_append, lxb_dom_element_set_attribute, lxb_dom_node_insert_child,
    lxb_ns_by_id,
};

const LXB_STATUS_OK: u32 = lxb::lexbor_status_t_LXB_STATUS_OK;

/// A DOM name or value slice must fit `u32` - the mkr arena's per-slice cap and
/// its factory signatures. A slice over 4 GiB is rejected fail-closed rather
/// than wrapped into a short one.
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

/// Intern `uri` in the DESTINATION document's namespace table and return its
/// Lexbor id, so an element's namespace survives translation for any URI - not
/// just the few Lexbor knows by default.
///
/// An empty URI, or an intern failure, is the null namespace. Fail-soft on
/// purpose: losing a namespace annotation is not the same as producing a wrong
/// tree, and the C did the same.
unsafe fn intern_ns(hdoc: *mut LxbDoc, uri: &[u8]) -> usize {
    if uri.is_empty() || (*hdoc).ns.is_null() {
        return NS_UNDEF;
    }
    let d = lxb::lxb_ns_append((*hdoc).ns as *mut c_void, uri.as_ptr(), uri.len());
    if d.is_null() {
        NS_UNDEF
    } else {
        (*d).ns_id
    }
}

/* ---- the work stack, shared by both directions ----
 *
 * `def` carries, for HTML->XML, the default-namespace URI in scope for the
 * destination node's CHILDREN, so a child only redeclares xmlns when it
 * differs. Unused for XML->HTML. */
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
///
/// `Some(None)` distinguishes "a template whose content is NULL" from "not a
/// template", because the two directions apply different rules to the first.
unsafe fn template_content(n: *const LxbNode) -> Option<*mut c_void> {
    if (*n).type_ == h::ELEMENT && (*n).local_name == TAG_TEMPLATE && (*n).ns == NS_HTML {
        return Some((*(n as *const lxb::lxb_html_template_element_t)).content as *mut c_void);
    }
    None
}

/* ================= HTML (lxb) -> XML (mkr) ========================== */

/// Declare `xmlns` (no prefix) or `xmlns:PREFIX` = `uri` on the detached mkr
/// element, as an ordinary attribute, so the subtree's prefix-based namespace
/// resolution at link time reproduces `uri`.
unsafe fn declare_ns(xdoc: *mut XmlDoc, el: *mut XmlNode, prefix: &[u8], uri: &[u8]) -> c_int {
    if prefix.is_empty() {
        return mutate::set_attribute(xdoc, el, b"xmlns", uri, core::ptr::null_mut());
    }
    /* "xmlns:" + prefix, built in a scratch buffer. */
    let nlen = match 6usize.checked_add(prefix.len()).and_then(fits_u32) {
        Some(_) => 6 + prefix.len(),
        None => return MUT_OOM,
    };
    let mut name: Vec<u8> = match try_vec_with_capacity(nlen) {
        Some(v) => v,
        None => return MUT_OOM,
    };
    name.extend_from_slice(b"xmlns:"); /* reserved above */
    name.extend_from_slice(prefix);
    mutate::set_attribute(xdoc, el, &name, uri, core::ptr::null_mut())
}

/// Copy the source element's attributes onto the translated mkr element,
/// declaring an `xmlns:PREFIX` for each foreign-prefixed one so resolution at
/// link time succeeds. The predefined `xml:` prefix needs none.
unsafe fn h2x_copy_attrs(xdoc: *mut XmlDoc, s: *mut LxbNode, el: *mut XmlNode) -> c_int {
    let mut a = lxb::lxb_dom_element_first_attribute_noi(s as *mut LxbElement);
    while !a.is_null() {
        let mut anl = 0usize;
        let mut avl = 0usize;
        let an = lxb::lxb_dom_attr_qualified_name(a, &mut anl);
        let av = lxb::lxb_dom_attr_value_noi(a, &mut avl);
        if fits_u32(anl).is_none() || fits_u32(avl).is_none() {
            return MUT_OOM;
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
                    let st = declare_ns(xdoc, el, &name[..colon], uri);
                    if st != MUT_OK {
                        return st;
                    }
                }
            }
        }

        let st = mutate::set_attribute(xdoc, el, name, value, core::ptr::null_mut());
        if st != MUT_OK {
            return st;
        }
        a = lxb::lxb_dom_element_next_attribute_noi(a);
    }
    MUT_OK
}

/// What [`h2x_make`] produced, plus the default namespace in scope for the new
/// node's children.
struct Made<'a> {
    node: *mut XmlNode,
    child_default: Option<&'a [u8]>,
}

/// Translate ONE Lexbor node into a fresh mkr node - its own fields and
/// attributes, NOT its children.
///
/// `node` is null to SKIP an unsupported type; an `Err` status fails the whole
/// import.
unsafe fn h2x_make<'a>(
    xdoc: *mut XmlDoc,
    s: *mut LxbNode,
    parent_default: Option<&'a [u8]>,
) -> Result<Made<'a>, c_int> {
    /* A non-element does not change the default-namespace scope. */
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
                return Err(MUT_OOM);
            }
            let name = core::slice::from_raw_parts(nm, nl);
            let euri = html_ns_uri(s);

            /* Strict first, so a valid QName stays XML-serializable.
             * importNode never re-validates an existing node's name - the DOM
             * requires the source to be well-formed for its own representation -
             * so an HTML name that is a valid DOM element name but NOT a
             * well-formed XML QName (":good:times:", "x<", "0:a") is taken
             * VERBATIM as an unprefixed DOM-loose name rather than rejected,
             * the same escape hatch as create_loose_dom_element. HTML elements
             * are always unprefixed. The namespace is passed DIRECTLY, because
             * link-time resolution skips loose names and a synthesized xmlns
             * declaration would never reach it. */
            let mut el: *mut XmlNode = core::ptr::null_mut();
            let mut st = mutate::new_element(xdoc, name, &mut el);
            if st == MUT_BAD_NAME && !name.is_empty() {
                let qn = QName {
                    qname: nm as *const c_char,
                    qname_len: nl as u32,
                    prefix: nm as *const c_char,
                    prefix_len: 0,
                    local: nm as *const c_char,
                    local_len: nl as u32,
                };
                st = mutate::new_loose_dom_element(xdoc, &qn, euri.unwrap_or(&[]), &mut el);
            }
            if st != MUT_OK {
                return Err(st);
            }

            /* Declare the default namespace iff it differs from the inherited
             * one, so this element (unprefixed, like all HTML elements) and its
             * children resolve to it. An element with no namespace under an
             * inherited default UNdeclares, with xmlns="". For a loose element
             * this only feeds child inheritance; its own ns_uri was set above. */
            let mut child_default = parent_default;
            if euri.unwrap_or(&[]) != parent_default.unwrap_or(&[]) {
                let st = declare_ns(xdoc, el, &[], euri.unwrap_or(&[]));
                if st != MUT_OK {
                    return Err(st);
                }
                child_default = Some(euri.unwrap_or(&[]));
            }

            let st = h2x_copy_attrs(xdoc, s, el);
            if st != MUT_OK {
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
                None => return Err(MUT_OOM),
            };
            let ty = match (*s).type_ {
                h::TEXT => X_TEXT,
                h::CDATA => X_CDATA,
                _ => X_COMMENT,
            };
            let text = if d.data.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(d.data, len as usize)
            };
            let mut out: *mut XmlNode = core::ptr::null_mut();
            let st = mutate::new_chardata(xdoc, ty, text, &mut out);
            if st != MUT_OK {
                return Err(st);
            }
            unchanged(out)
        }

        h::PI => {
            let mut tl = 0usize;
            let tg = lxb::lxb_dom_processing_instruction_target_noi(s as *mut _, &mut tl);
            let d = &(*(s as *const lxb::lxb_dom_character_data_t)).data;
            if fits_u32(tl).is_none() || fits_u32(d.length).is_none() {
                return Err(MUT_OOM);
            }
            let target = core::slice::from_raw_parts(tg, tl);
            let data = if d.data.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(d.data, d.length)
            };
            let mut out: *mut XmlNode = core::ptr::null_mut();
            let st = mutate::new_pi(xdoc, target, data, &mut out);
            if st != MUT_OK {
                return Err(st);
            }
            unchanged(out)
        }

        h::FRAGMENT => {
            let f = crate::xml::arena::arena_node(xdoc, X_FRAGMENT);
            if f.is_null() {
                return Err(MUT_OOM);
            }
            unchanged(f)
        }

        /* An unsupported descendant type is skipped, not an error. */
        _ => unchanged(core::ptr::null_mut()),
    }
}

/// The children to translate under `s`.
///
/// An HTML `<template>` keeps its content in a SEPARATE fragment, not the normal
/// child chain, so a plain `first_child` walk would silently drop it. The
/// content fragment is descended into instead: the XML side has no
/// template-content concept, so the contents become ordinary children of the
/// translated element - lossless, and the natural XML shape.
unsafe fn h2x_children_of(s: *mut LxbNode) -> *mut LxbNode {
    match template_content(s) {
        Some(content) if !content.is_null() => (*(content as *mut LxbNode)).first_child,
        Some(_) => core::ptr::null_mut(),
        None => (*s).first_child,
    }
}

/// Deep- or shallow-copy an HTML subtree into the XML arena, detached.
pub unsafe extern "C" fn mkr_cross_html_to_xml(
    xdoc: *mut XmlDoc,
    src: *mut LxbNode,
    deep: c_int,
    out: *mut *mut XmlNode,
) -> c_int {
    *out = core::ptr::null_mut();

    let root = match h2x_make(xdoc, src, None) {
        Ok(m) => m,
        Err(st) => return st,
    };
    if root.node.is_null() {
        return MUT_TYPE; /* the root's type has no XML counterpart */
    }

    if deep != 0 {
        let mut stack: Vec<Frame<*mut LxbNode, *mut XmlNode>> = match try_vec_with_capacity(1) {
            Some(v) => v,
            None => return MUT_OOM,
        };
        /* The borrow is over Lexbor's interned namespace table, which lives as
         * long as the source document - longer than this call. `Frame` says
         * 'static because there is no lifetime here to tie it to; the arena
         * outliving the walk is the real guarantee. */
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
            return MUT_OOM;
        }

        while let Some(f) = stack.pop() {
            let mut c = h2x_children_of(f.s);
            while !c.is_null() {
                let made = match h2x_make(xdoc, c, f.def) {
                    Ok(m) => m,
                    Err(st) => return st, /* partial subtree abandoned in the arena */
                };
                if !made.node.is_null() {
                    /* The parent is detached, so namespace resolution is
                     * deferred to the eventual link. */
                    let st = mutate::insert_child(xdoc, f.d, made.node);
                    if st != MUT_OK {
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
                            return MUT_OOM;
                        }
                    }
                }
                c = (*c).next;
            }
        }
    }

    *out = root.node;
    MUT_OK
}

/* ================= XML (mkr) -> HTML (lxb) ========================== */

/// Copy the source element's attributes onto the translated Lexbor element,
/// preserving each attribute's namespace: a null-namespace one through
/// `set_attribute`, a namespaced one through an explicit
/// `lxb_dom_attr_set_name_ns`.
unsafe fn x2h_copy_attrs(hdoc: *mut LxbDoc, s: *const XmlNode, el: *mut LxbElement) -> c_int {
    let mut a = (*s).attrs;
    while !a.is_null() {
        let val = if (*a).value.is_null() {
            &[][..]
        } else {
            core::slice::from_raw_parts((*a).value as *const u8, (*a).value_len as usize)
        };
        let qname = core::slice::from_raw_parts((*a).qname as *const u8, (*a).qname_len as usize);

        if (*a).ns_uri_len == 0 {
            if lxb_dom_element_set_attribute(
                el,
                qname.as_ptr(),
                qname.len(),
                val.as_ptr(),
                val.len(),
            )
            .is_null()
            {
                return MUT_OOM;
            }
        } else {
            let at = lxb_dom_attr_interface_create(hdoc);
            if at.is_null() {
                return MUT_OOM;
            }
            let ns =
                core::slice::from_raw_parts((*a).ns_uri as *const u8, (*a).ns_uri_len as usize);
            if lxb::lxb_dom_attr_set_name_ns(
                at,
                ns.as_ptr(),
                ns.len(),
                qname.as_ptr(),
                qname.len(),
                false,
            ) != LXB_STATUS_OK
                || lxb_dom_attr_set_value(at, val.as_ptr(), val.len()) != LXB_STATUS_OK
            {
                /* The un-appended attr is abandoned in mraw, freed with the
                 * document - this module's fail-closed model. */
                return MUT_OOM;
            }
            lxb_dom_element_attr_append(el, at);
        }
        a = (*a).next;
    }
    MUT_OK
}

/// Translate ONE mkr node into a fresh, detached Lexbor node - its own fields
/// and attributes, NOT children.
///
/// Null to SKIP an unsupported type. An XML CDATA section has no HTML
/// counterpart, so it fails closed rather than degrading to a text node.
unsafe fn x2h_make(hdoc: *mut LxbDoc, s: *const XmlNode) -> Result<*mut LxbNode, c_int> {
    let value = || -> &[u8] {
        if (*s).value.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts((*s).value as *const u8, (*s).value_len as usize)
        }
    };

    match (*s).type_ {
        X_ELEMENT => {
            let qname =
                core::slice::from_raw_parts((*s).qname as *const u8, (*s).qname_len as usize);
            let el = lxb_dom_document_create_element(
                hdoc,
                qname.as_ptr(),
                qname.len(),
                core::ptr::null_mut(),
            );
            if el.is_null() {
                return Err(MUT_OOM);
            }
            /* Preserve the namespace as a Lexbor id, interning any URI. */
            let ns = if (*s).ns_uri_len == 0 {
                &[][..]
            } else {
                core::slice::from_raw_parts((*s).ns_uri as *const u8, (*s).ns_uri_len as usize)
            };
            (*(el as *mut LxbNode)).ns = intern_ns(hdoc, ns);

            let st = x2h_copy_attrs(hdoc, s, el);
            if st != MUT_OK {
                return Err(st);
            }
            Ok(el as *mut LxbNode)
        }

        X_TEXT => {
            let v = value();
            let t = lxb_dom_document_create_text_node(hdoc, v.as_ptr(), v.len());
            if t.is_null() {
                return Err(MUT_OOM);
            }
            Ok(t as *mut LxbNode)
        }

        X_COMMENT => {
            let v = value();
            let c = lxb_dom_document_create_comment(hdoc, v.as_ptr(), v.len());
            if c.is_null() {
                return Err(MUT_OOM);
            }
            Ok(c as *mut LxbNode)
        }

        X_PI => {
            /* A PI's target is its name (local == qname); its data is the value. */
            let target =
                core::slice::from_raw_parts((*s).local as *const u8, (*s).local_len as usize);
            let v = value();
            let pi = lxb_dom_document_create_processing_instruction(
                hdoc,
                target.as_ptr(),
                target.len(),
                v.as_ptr(),
                v.len(),
            );
            if pi.is_null() {
                return Err(MUT_OOM);
            }
            Ok(pi as *mut LxbNode)
        }

        X_CDATA => Err(MUT_TYPE), /* HTML has no CDATA section */

        X_FRAGMENT => {
            let f = lxb_dom_document_create_document_fragment(hdoc);
            if f.is_null() {
                return Err(MUT_OOM);
            }
            Ok(f as *mut LxbNode)
        }

        _ => Ok(core::ptr::null_mut()), /* unsupported descendant type: skip */
    }
}

/// Where a translated element's CHILDREN attach.
///
/// An HTML `<template>` holds its content in a separate fragment, not the normal
/// child chain, so children go there - matching a parsed template and the
/// HTML->HTML import fixup. Other elements take children directly.
unsafe fn x2h_link_target(el: *mut LxbNode) -> *mut LxbNode {
    match template_content(el) {
        Some(content) if !content.is_null() => content as *mut LxbNode,
        _ => el,
    }
}

/// Deep- or shallow-copy an XML subtree into the Lexbor arena, detached.
pub unsafe extern "C" fn mkr_cross_xml_to_html(
    hdoc: *mut LxbDoc,
    src: *const XmlNode,
    deep: c_int,
    out: *mut *mut LxbNode,
) -> c_int {
    *out = core::ptr::null_mut();

    let root = match x2h_make(hdoc, src) {
        Ok(n) => n,
        Err(st) => return st,
    };
    if root.is_null() {
        return MUT_TYPE; /* the root's type has no HTML counterpart */
    }

    if deep != 0 {
        let mut stack: Vec<Frame<*const XmlNode, *mut LxbNode>> = match try_vec_with_capacity(1) {
            Some(v) => v,
            None => return MUT_OOM,
        };
        /* The frame's `d` is the LINK TARGET for the source node's children: a
         * template element's content fragment, else the element itself. */
        if push(
            &mut stack,
            Frame {
                s: src,
                d: x2h_link_target(root),
                def: None,
            },
        )
        .is_err()
        {
            return MUT_OOM;
        }

        while let Some(f) = stack.pop() {
            let mut c = (*f.s).first_child as *const XmlNode;
            while !c.is_null() {
                let dc = match x2h_make(hdoc, c) {
                    Ok(n) => n,
                    Err(st) => return st, /* partial subtree abandoned in mraw */
                };
                if !dc.is_null() {
                    lxb_dom_node_insert_child(f.d, dc);
                    if !(*c).first_child.is_null()
                        && push(
                            &mut stack,
                            Frame {
                                s: c,
                                d: x2h_link_target(dc),
                                def: None,
                            },
                        )
                        .is_err()
                    {
                        return MUT_OOM;
                    }
                }
                c = (*c).next as *const XmlNode;
            }
        }
    }

    *out = root;
    MUT_OK
}
