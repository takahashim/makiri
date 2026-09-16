//! The one place Makiri reads Lexbor's DOM - node, element, attribute and
//! document fields, and the Lexbor accessors over them - and, through
//! [`HtmlNodeMut`], the one place it edits the tree.
//!
//! Everything here reads the GENERATED layout (`crate::lexbor_abi`), so there is
//! no hand-written copy of a Lexbor struct left to drift from the pinned headers.
//! The XPath engine's HTML backend and the Ruby-facing readers both come through
//! this module, so a field is read one way, in one place.
//!
//! The functions take raw handles for now. Each states its contract once: the
//! handle is live, and the document it belongs to is not being changed while a
//! borrowed slice is in use - which `glue::doc::DocumentEvaluation` enforces for
//! the one place Ruby can run mid-read, an XPath handler.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::lexbor_abi::{self as lxb, LxbAttr, LxbDoc, LxbElement, LxbNode};

/* A node handle is cast to an element or attribute handle, which is sound only
 * while the node sits FIRST in both. That is a claim about the absolute offset,
 * so it is asserted as zero: if Lexbor ever put a field ahead of `node`, every
 * such cast would become wrong. */
const _: () = assert!(
    core::mem::offset_of!(lxb::lxb_dom_element_t, node) == 0,
    "lxb_dom_element_t no longer starts with its node - the handle cast is unsound"
);
const _: () = assert!(
    core::mem::offset_of!(lxb::lxb_dom_attr_t, node) == 0,
    "lxb_dom_attr_t no longer starts with its node - the handle cast is unsound"
);

/* ---- the Lexbor constants the readers compare against ----
 *
 * Generated, not restated. LXB_NS_HTML is 2, and a hand-written 1 once made
 * every HTML element foreign, so every unprefixed name test matched nothing -
 * silently. Deriving the value removes the class rather than checking for it. */
pub const NS_UNDEF: usize = lxb::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
pub const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
/// The two foreign roots. A fragment parsed in one of their contexts follows
/// the foreign-content rules rather than the HTML ones.
pub const NS_SVG: usize = lxb::lxb_ns_id_enum_t_LXB_NS_SVG as usize;
/// See [`NS_SVG`].
pub const NS_MATH: usize = lxb::lxb_ns_id_enum_t_LXB_NS_MATH as usize;
/// `LXB_NS_XML`. An attribute in it keeps its `xml:` prefix across a
/// cross-document translation rather than having one invented.
pub const NS_XML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_XML as usize;

/// `LXB_TAG__UNDEF`. A custom element's tag id is a pointer value, far above
/// the static range the element index buckets, so it is compared against
/// [`TAG_LAST_ENTRY`] rather than this.
pub const TAG_UNDEF: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize;

/// `LXB_TAG__LAST_ENTRY` - the end of Lexbor's static tag-id range.
pub const TAG_LAST_ENTRY: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__LAST_ENTRY as usize;

/// The three tags a fragment context can be named by: `<body>` is the default
/// context, and `<svg>`/`<math>` are the foreign roots.
pub const TAG_BODY: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_BODY as usize;
/// See [`TAG_BODY`].
pub const TAG_SVG: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_SVG as usize;
/// See [`TAG_BODY`].
pub const TAG_MATH: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_MATH as usize;

/// The last of Lexbor's special tag ids (text, comment, doctype, document,
/// eof). A token at or below it is not an element start-tag.
pub const TAG_EM_DOCTYPE: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__EM_DOCTYPE as usize;

/* ---------- borrowed bytes ---------- */

#[inline]
unsafe fn seen<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p, len)
    }
}

#[inline]
unsafe fn named<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*const T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0;
    seen(f(h, &mut len), len)
}

#[inline]
unsafe fn named_mut<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*mut T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0;
    seen(f(h, &mut len), len)
}

/* ---------- navigation ---------- */

/// The document's root node handle. An `lxb_dom_document_t` leads with its
/// node, so this is a cast to the embedded base.
#[inline]
pub unsafe fn document_node(doc: *mut LxbDoc) -> *mut LxbNode {
    doc as *mut LxbNode
}

/// A node's type (`lxb_dom_node_type_t`).
#[inline]
pub unsafe fn node_type(node: *mut LxbNode) -> u32 {
    (*node).type_
}
#[inline]
pub unsafe fn first_child(node: *mut LxbNode) -> *mut LxbNode {
    (*node).first_child
}
#[inline]
pub unsafe fn last_child(node: *mut LxbNode) -> *mut LxbNode {
    (*node).last_child
}
#[inline]
pub unsafe fn next(node: *mut LxbNode) -> *mut LxbNode {
    (*node).next
}
#[inline]
pub unsafe fn prev(node: *mut LxbNode) -> *mut LxbNode {
    (*node).prev
}
#[inline]
pub unsafe fn parent(node: *mut LxbNode) -> *mut LxbNode {
    (*node).parent
}

/* ---------- attributes ---------- */

/// `element` must be an element node.
#[inline]
pub unsafe fn first_attr(element: *mut LxbNode) -> *mut LxbNode {
    (*(element as *mut LxbElement)).first_attr as *mut LxbNode
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_next(attr: *mut LxbNode) -> *mut LxbNode {
    (*(attr as *mut LxbAttr)).next as *mut LxbNode
}
/// An attribute's value, borrowed from the document.
#[inline]
pub unsafe fn attr_value<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named_mut(attr as *mut LxbAttr, lxb::lxb_dom_attr_value_noi)
}
/// The value of `element`'s attribute named `name` (Lexbor's lookup), or None.
#[inline]
pub unsafe fn get_attribute<'a>(element: *mut LxbNode, name: &[u8]) -> Option<&'a [u8]> {
    let mut len = 0;
    let value = lxb::lxb_dom_element_get_attribute(
        element as *mut LxbElement,
        name.as_ptr(),
        name.len(),
        &mut len,
    );
    if value.is_null() {
        None
    } else {
        Some(seen(value, len))
    }
}

/* ---------- names ---------- */

/// `node` must be an element node.
#[inline]
pub unsafe fn local_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    named_mut(node as *mut LxbElement, lxb::lxb_dom_element_local_name)
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_local_name<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb::lxb_dom_attr_local_name)
}
#[inline]
pub unsafe fn qualified_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    if (*node).type_ == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT {
        named(node as *mut LxbElement, lxb::lxb_dom_element_qualified_name)
    } else {
        named_mut(node, lxb::lxb_dom_node_name)
    }
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_qualified_name<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb::lxb_dom_attr_qualified_name)
}
#[inline]
pub unsafe fn pi_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    named_mut(node, lxb::lxb_dom_node_name)
}

/* ---------- namespaces ---------- */

/// The node's namespace URI, borrowed from its document's namespace table, or
/// empty when it has none.
pub unsafe fn ns_uri<'a>(node: *mut LxbNode) -> &'a [u8] {
    if node.is_null() || (*node).ns == NS_UNDEF {
        return &[];
    }
    let doc = (*node).owner_document;
    if doc.is_null() || (*doc).ns.is_null() {
        return &[];
    }
    let mut len = 0;
    seen(lxb::lxb_ns_by_id((*doc).ns, (*node).ns, &mut len), len)
}
/// Whether an element is in a namespace other than HTML's.
#[inline]
pub unsafe fn is_foreign_ns(node: *mut LxbNode) -> bool {
    (*node).ns != NS_HTML && (*node).ns != NS_UNDEF
}
#[inline]
pub unsafe fn has_ns(node: *mut LxbNode) -> bool {
    (*node).ns != NS_UNDEF
}

/* ---------- documents ---------- */

/// A tag name as Lexbor's tag id, for the `//tag` index, or [`TAG_UNDEF`].
pub unsafe fn tag_id_by_name(doc: *const LxbDoc, local: &[u8]) -> usize {
    if doc.is_null() || local.is_empty() || (*doc).tags.is_null() {
        return TAG_UNDEF;
    }
    lxb::lxb_tag_id_by_name_noi((*doc).tags, local.as_ptr(), local.len())
}

/* ---------- text ---------- */

/* ------------------------------------------------------------------ *
 * typed handles                                                      *
 * ------------------------------------------------------------------ */

/* The readers above take raw handles and state their contract per call. The
 * handles below state it once: holding one IS the proof that the node is live
 * and that its document is neither freed nor restructured while `'doc` lasts.
 * `HtmlNode::from_raw` is the only way to make one without already holding
 * one, so it is the single place that contract is asserted, and every method
 * is safe.
 *
 * "Not restructured" admits one write: building the attribute->owner index
 * backfills an attribute's `parent` from null to its element. That is why the
 * methods read fields through the raw pointer, place by place, rather than
 * holding a `&LxbNode` - no reference to a Lexbor struct outlives the read. */

/* The node types the handles branch on, generated. */
pub const TYPE_ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
pub const TYPE_ATTRIBUTE: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ATTRIBUTE;
pub const TYPE_TEXT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT;
pub const TYPE_CDATA: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION;
pub const TYPE_PI: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION;
pub const TYPE_DOCUMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT;
pub const TYPE_COMMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT;
pub const TYPE_DOCTYPE: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;
pub const TYPE_FRAGMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;

/// `LXB_TAG_TEMPLATE`.
pub const TAG_TEMPLATE: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_TEMPLATE as usize;

/// A node of a live Lexbor document, borrowed for `'doc`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlNode<'doc> {
    raw: NonNull<LxbNode>,
    _doc: PhantomData<&'doc LxbNode>,
}

/// An element node.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlElement<'doc>(HtmlNode<'doc>);

/// An attribute node.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlAttr<'doc>(HtmlNode<'doc>);

/// A live Lexbor document, borrowed for `'doc`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlDoc<'doc> {
    raw: NonNull<LxbDoc>,
    _doc: PhantomData<&'doc LxbDoc>,
}

impl<'doc> HtmlDoc<'doc> {
    /// # Safety
    /// `raw` must be null or a live document that outlives `'doc` and is not
    /// restructured (see the section note) while `'doc` lasts.
    #[inline]
    pub unsafe fn from_raw(raw: *mut LxbDoc) -> Option<Self> {
        NonNull::new(raw).map(|raw| HtmlDoc {
            raw,
            _doc: PhantomData,
        })
    }

    #[inline]
    pub fn as_raw(self) -> *mut LxbDoc {
        self.raw.as_ptr()
    }

    /// The document as a node: an `lxb_dom_document_t` leads with its node.
    #[inline]
    pub fn as_node(self) -> HtmlNode<'doc> {
        HtmlNode {
            raw: self.raw.cast(),
            _doc: PhantomData,
        }
    }

    /// A detached element named `local_name`, in no namespace yet.
    ///
    /// `None` when Lexbor could not make one. The DOM's createElement: the
    /// result belongs to this document but is in no tree, which is what
    /// [`BuildingElement`] says.
    pub fn create_element(self, local_name: &[u8]) -> Option<BuildingElement<'doc>> {
        // SAFETY: a live document; Lexbor copies the name into its own storage.
        unsafe {
            BuildingElement::from_raw(lxb::lxb_dom_document_create_element(
                self.as_raw(),
                local_name.as_ptr(),
                local_name.len(),
                core::ptr::null_mut(),
            ))
        }
    }

    /// A detached text node holding `text`. `None` on allocation failure.
    pub fn create_text(self, text: &[u8]) -> Option<BuildingNode<'doc>> {
        // SAFETY: a live document; Lexbor copies the bytes.
        unsafe {
            BuildingNode::from_raw(lxb::lxb_dom_document_create_text_node(
                self.as_raw(),
                text.as_ptr(),
                text.len(),
            ) as *mut LxbNode)
        }
    }

    /// A detached comment holding `text`. `None` on allocation failure.
    pub fn create_comment(self, text: &[u8]) -> Option<BuildingNode<'doc>> {
        // SAFETY: as above.
        unsafe {
            BuildingNode::from_raw(lxb::lxb_dom_document_create_comment(
                self.as_raw(),
                text.as_ptr(),
                text.len(),
            ) as *mut LxbNode)
        }
    }

    /// A detached processing instruction, or `None` when Lexbor could not make
    /// one. That includes an invalid target, which Lexbor validates, so this
    /// fails closed rather than building a PI that cannot serialize.
    pub fn create_pi(self, target: &[u8], data: &[u8]) -> Option<BuildingNode<'doc>> {
        // SAFETY: as above, for both slices.
        unsafe {
            BuildingNode::from_raw(lxb::lxb_dom_document_create_processing_instruction(
                self.as_raw(),
                target.as_ptr(),
                target.len(),
                data.as_ptr(),
                data.len(),
            ) as *mut LxbNode)
        }
    }

    /// A detached DocumentType, DOM `createDocumentType`. `None` on allocation
    /// failure; the caller validates `name` first, since an invalid one is the
    /// caller's error rather than Lexbor's.
    ///
    /// `None` for an id means ABSENT, and that is not the same as an empty one:
    /// Lexbor reads a null pointer as omitted, so `Some(&[])` would be a
    /// present, empty id.
    ///
    /// Two things Lexbor leaves to the caller are done here, because both are
    /// facts about its DOM rather than about the caller:
    ///
    /// * `create` interns the name through the attribute local-name hash, which
    ///   ASCII-lowercases it, while the DOM preserves case. So the name is
    ///   re-interned case-preserving and the doctype repointed at that.
    /// * `create` leaves an absent id as a `{NULL, 0}` string, and
    ///   `lxb_dom_document_type_interface_clone` - which `import_node` and
    ///   `clone_node` reach - runs `lexbor_str_copy`, which fails on a NULL
    ///   source. Such a doctype would be unimportable, so an absent id is
    ///   initialised to an allocated empty string instead. Length stays 0, so
    ///   the accessor and the serializer still read it as absent.
    pub fn create_doctype(
        self,
        name: &[u8],
        public_id: Option<&[u8]>,
        system_id: Option<&[u8]>,
    ) -> Option<BuildingNode<'doc>> {
        let part = |id: Option<&[u8]>| match id {
            Some(b) if !b.is_empty() => (b.as_ptr(), b.len()),
            _ => (core::ptr::null(), 0),
        };
        let (pub_ptr, pub_len) = part(public_id);
        let (sys_ptr, sys_len) = part(system_id);

        // SAFETY: a live document; Lexbor copies every slice it keeps. The
        // exception code is written and not read - a null result is the failure
        // signal, as it was in the C.
        unsafe {
            let mut code: core::ffi::c_int = 0;
            let dt = lxb::lxb_dom_document_type_create(
                self.as_raw(),
                name.as_ptr(),
                name.len(),
                pub_ptr,
                pub_len,
                sys_ptr,
                sys_len,
                &mut code,
            );
            if dt.is_null() {
                return None;
            }

            let interned = lxb::lxb_dom_attr_qualified_name_append(
                (*self.as_raw()).attrs as *mut core::ffi::c_void,
                name.as_ptr(),
                name.len(),
            );
            if interned.is_null() {
                /* The doctype is left for the arena, like any other half-built
                 * node this crate abandons. */
                return None;
            }
            (*dt).name = (*interned).attr_id;

            let text = (*self.as_raw()).text;
            if (*dt).public_id.data.is_null() {
                lxb::lexbor_str_init(&mut (*dt).public_id, text, 0);
            }
            if (*dt).system_id.data.is_null() {
                lxb::lexbor_str_init(&mut (*dt).system_id, text, 0);
            }
            BuildingNode::from_raw(dt as *mut LxbNode)
        }
    }

    /// Whether `name` satisfies the DOM's doctype-name production, which
    /// [`create_doctype`](Self::create_doctype) requires of its caller.
    ///
    /// Lexbor's check is a scan for the bytes a doctype name may not hold -
    /// whitespace, NUL and `>` - so this reads the slice and touches no
    /// document. An empty name is rejected without asking, because Lexbor reads
    /// a null pointer as absent and an empty Rust slice's pointer is not null.
    pub fn valid_doctype_name(name: &[u8]) -> bool {
        if name.is_empty() {
            return false;
        }
        // SAFETY: a slice the caller holds; Lexbor only reads it.
        unsafe { lxb::lxb_dom_document_type_valid_name(name.as_ptr(), name.len()) }
    }

    /// An empty detached DocumentFragment. `None` on allocation failure.
    pub fn create_fragment(self) -> Option<BuildingNode<'doc>> {
        // SAFETY: a live document.
        unsafe {
            BuildingNode::from_raw(
                lxb::lxb_dom_document_create_document_fragment(self.as_raw()) as *mut LxbNode,
            )
        }
    }

    /// Copy `src` into this document, DOM `importNode`. `None` when Lexbor
    /// could not.
    ///
    /// The copy is detached and belongs here, which is what [`BuildingNode`]
    /// says. `deep` carries the subtree - but NOT a `<template>`'s separate
    /// contents fragment, which Lexbor's importNode omits; the caller fixes
    /// that up (see `glue::fragment::import_with_fixup`).
    pub fn import_node(self, src: HtmlNode<'_>, deep: bool) -> Option<BuildingNode<'doc>> {
        // SAFETY: two live documents' nodes; Lexbor allocates the copy in this
        // one and leaves the source alone.
        unsafe {
            BuildingNode::from_raw(lxb::lxb_dom_document_import_node(
                self.as_raw(),
                src.as_raw(),
                deep,
            ))
        }
    }

    /// Intern `uri` in the document's namespace table and give back its id -
    /// the half of the key the DOM matches a namespaced attribute on.
    ///
    /// [`NS_UNDEF`] for an empty URI, and for a document with no table or one
    /// that could not take another entry: a miss then simply finds nothing,
    /// rather than matching the wrong attribute.
    pub fn intern_ns(self, uri: &[u8]) -> usize {
        if uri.is_empty() {
            return NS_UNDEF;
        }
        // SAFETY: a live document; the table and the URI are only read, and
        // Lexbor copies the URI into its own storage.
        unsafe {
            let table = (*self.as_raw()).ns;
            if table.is_null() {
                return NS_UNDEF;
            }
            let d = lxb::lxb_ns_append(table as *mut core::ffi::c_void, uri.as_ptr(), uri.len());
            if d.is_null() {
                NS_UNDEF
            } else {
                (*d).ns_id
            }
        }
    }
}

impl<'doc> HtmlNode<'doc> {
    /// # Safety
    /// `raw` must be null or a live node whose document outlives `'doc` and is
    /// not restructured (see the section note) while `'doc` lasts.
    #[inline]
    pub unsafe fn from_raw(raw: *mut LxbNode) -> Option<Self> {
        NonNull::new(raw).map(|raw| HtmlNode {
            raw,
            _doc: PhantomData,
        })
    }

    /// A link read out of a node held for `'doc` leads to a node of the same
    /// tree, which lives as long.
    #[inline]
    fn link(p: *mut LxbNode) -> Option<Self> {
        // SAFETY: `p` is a field of a node live for 'doc; Lexbor's tree links
        // stay within the document, and a detached node's links are null.
        unsafe { Self::from_raw(p) }
    }

    #[inline]
    pub fn as_raw(self) -> *mut LxbNode {
        self.raw.as_ptr()
    }

    /// The node's type (`lxb_dom_node_type_t`).
    #[inline]
    pub fn node_type(self) -> u32 {
        // SAFETY: a live node (the handle's contract).
        unsafe { (*self.as_raw()).type_ }
    }

    #[inline]
    pub fn parent(self) -> Option<Self> {
        // SAFETY: as `node_type`.
        Self::link(unsafe { (*self.as_raw()).parent })
    }
    #[inline]
    pub fn first_child(self) -> Option<Self> {
        // SAFETY: as `node_type`.
        Self::link(unsafe { (*self.as_raw()).first_child })
    }
    #[inline]
    pub fn last_child(self) -> Option<Self> {
        // SAFETY: as `node_type`.
        Self::link(unsafe { (*self.as_raw()).last_child })
    }
    #[inline]
    pub fn next(self) -> Option<Self> {
        // SAFETY: as `node_type`.
        Self::link(unsafe { (*self.as_raw()).next })
    }
    #[inline]
    pub fn prev(self) -> Option<Self> {
        // SAFETY: as `node_type`.
        Self::link(unsafe { (*self.as_raw()).prev })
    }

    /// The children, first to last.
    pub fn children(self) -> Siblings<'doc> {
        Siblings(self.first_child())
    }
    /// The ancestors, nearest first.
    pub fn ancestors(self) -> Ancestors<'doc> {
        Ancestors(self.parent())
    }

    /// The node after this one in a pre-order walk of `root`'s subtree, or
    /// None past the last. Climbs by parent links rather than recursing, so an
    /// adversarially deep tree cannot exhaust the stack; a node outside
    /// `root`'s subtree ends the walk instead of running off the tree.
    pub fn preorder_next(self, root: Self) -> Option<Self> {
        if let Some(c) = self.first_child() {
            return Some(c);
        }
        let mut n = self;
        loop {
            if n == root {
                return None;
            }
            if let Some(s) = n.next() {
                return Some(s);
            }
            n = n.parent()?;
        }
    }

    #[inline]
    pub fn element(self) -> Option<HtmlElement<'doc>> {
        (self.node_type() == TYPE_ELEMENT).then_some(HtmlElement(self))
    }
    #[inline]
    pub fn attr(self) -> Option<HtmlAttr<'doc>> {
        (self.node_type() == TYPE_ATTRIBUTE).then_some(HtmlAttr(self))
    }

    /// Lexbor's node name (DOM `nodeName`).
    pub fn node_name(self) -> &'doc [u8] {
        // SAFETY: a live node; the name is interned in its document.
        unsafe { named_mut(self.as_raw(), lxb::lxb_dom_node_name) }
    }

    /// The interned namespace id; [`NS_UNDEF`] for none.
    #[inline]
    pub fn ns_id(self) -> usize {
        // SAFETY: as `node_type`.
        unsafe { (*self.as_raw()).ns }
    }

    /// The interned tag id (`local_name`).
    #[inline]
    pub fn tag_id(self) -> usize {
        // SAFETY: as `node_type`.
        unsafe { (*self.as_raw()).local_name }
    }

    /// The namespace URI, or None when the node has none.
    pub fn ns_uri(self) -> Option<&'doc [u8]> {
        // SAFETY: a live node; the URI is interned in its document.
        let uri = unsafe { ns_uri(self.as_raw()) };
        (!uri.is_empty()).then_some(uri)
    }

    /// The document the node belongs to, as Lexbor's handle.
    #[inline]
    pub fn owner_document(self) -> *mut LxbDoc {
        // SAFETY: as `node_type`.
        unsafe { (*self.as_raw()).owner_document }
    }

    /// Whether both nodes belong to the same document.
    pub fn same_document(self, other: HtmlNode<'_>) -> bool {
        // SAFETY: both are live nodes.
        unsafe { (*self.as_raw()).owner_document == (*other.as_raw()).owner_document }
    }

    /// A processing instruction's target, or None for any other kind.
    pub fn pi_target(self) -> Option<&'doc [u8]> {
        (self.node_type() == TYPE_PI).then(|| {
            // SAFETY: a live PI node, which Lexbor allocates as one.
            unsafe {
                named_mut(
                    self.as_raw() as *mut lxb::lxb_dom_processing_instruction_t,
                    lxb::lxb_dom_processing_instruction_target_noi,
                )
            }
        })
    }

    /// A doctype's public id, or None for any other kind or an empty id.
    pub fn doctype_public_id(self) -> Option<&'doc [u8]> {
        self.doctype_id(lxb::lxb_dom_document_type_public_id_noi)
    }
    /// A doctype's system id, or None for any other kind or an empty id.
    pub fn doctype_system_id(self) -> Option<&'doc [u8]> {
        self.doctype_id(lxb::lxb_dom_document_type_system_id_noi)
    }
    fn doctype_id(
        self,
        f: unsafe extern "C" fn(*mut lxb::lxb_dom_document_type_t, *mut usize) -> *const u8,
    ) -> Option<&'doc [u8]> {
        if self.node_type() != TYPE_DOCTYPE {
            return None;
        }
        // SAFETY: a live doctype node, which Lexbor allocates as one.
        let id = unsafe { named_mut(self.as_raw() as *mut lxb::lxb_dom_document_type_t, f) };
        (!id.is_empty()).then_some(id)
    }

    /// A text or CDATA node's data, or None for any other kind.
    pub fn char_data(self) -> Option<&'doc [u8]> {
        if !matches!(self.node_type(), TYPE_TEXT | TYPE_CDATA) {
            return None;
        }
        // SAFETY: a live character-data node, which Lexbor allocates as one.
        unsafe {
            let cd = self.as_raw() as *mut lxb::lxb_dom_character_data_t;
            Some(seen((*cd).data.data, (*cd).data.length))
        }
    }

    /// Lexbor's text content of this node, lent to `f` - None when Lexbor has
    /// none - and freed once `f` returns.
    pub fn with_text_content<R>(self, f: impl FnOnce(Option<&[u8]>) -> R) -> R {
        let mut len = 0usize;
        // SAFETY: a live node.
        let t = unsafe { lxb::lxb_dom_node_text_content(self.as_raw(), &mut len) };
        if t.is_null() {
            return f(None);
        }
        // SAFETY: Lexbor handed back `len` bytes it owns until destroyed below.
        let r = f(Some(unsafe { core::slice::from_raw_parts(t, len) }));
        // SAFETY: `t` came from this node's document and is released once.
        unsafe { lxb::lxb_dom_document_destroy_text_noi((*self.as_raw()).owner_document, t) };
        r
    }

    /// An HTML `<template>` element's contents fragment, or None.
    pub fn template_content(self) -> Option<HtmlNode<'doc>> {
        if self.node_type() != TYPE_ELEMENT
            || self.tag_id() != TAG_TEMPLATE
            || self.ns_id() != NS_HTML
        {
            return None;
        }
        // SAFETY: an HTML-namespace `template` element is allocated as a
        // template element; its contents fragment is owned by the document.
        Self::link(unsafe {
            (*(self.as_raw() as *mut lxb::lxb_html_template_element_t)).content as *mut LxbNode
        })
    }

    /// A document node's root element, or None.
    pub fn document_root(self) -> Option<HtmlNode<'doc>> {
        if self.node_type() != TYPE_DOCUMENT {
            return None;
        }
        // SAFETY: a live document node, which leads its document struct.
        Self::link(unsafe { lxb::lxb_dom_document_root(self.as_raw() as *mut LxbDoc) })
    }
}

/// A node the caller has cleared for editing.
///
/// Editing a tree is not something any handle should be able to do: a frozen
/// receiver must refuse, and a document an XPath handler is evaluating over must
/// refuse too, because the engine borrows names and index slices across the walk
/// (see `glue::doc::DocumentEvaluation`). Those two checks live in one place,
/// `glue::html_node::mutate::unwrap_mutable`, and this type is what that place
/// hands back - so a node that has not been through them has no edit to call.
///
/// Reading needs no such clearance, so [`node`](Self::node) goes back down to
/// the ordinary handle, and the links below carry the clearance to a node of the
/// same document, which the same two checks covered.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlNodeMut<'doc>(HtmlNode<'doc>);

impl<'doc> HtmlNodeMut<'doc> {
    /// # Safety
    /// The caller has established that `node`'s document may be changed now:
    /// the receiver is not frozen, and no XPath evaluation is reading it.
    #[inline]
    pub unsafe fn assume_mutable(node: HtmlNode<'doc>) -> Self {
        HtmlNodeMut(node)
    }

    /// The node as an ordinary handle, for reading.
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }

    #[inline]
    pub fn as_raw(self) -> *mut LxbNode {
        self.0.as_raw()
    }

    /* The links: same document, so the caller's clearance covers them too. */

    #[inline]
    pub fn parent(self) -> Option<Self> {
        self.0.parent().map(HtmlNodeMut)
    }

    #[inline]
    pub fn first_child(self) -> Option<Self> {
        self.0.first_child().map(HtmlNodeMut)
    }

    #[inline]
    pub fn next(self) -> Option<Self> {
        self.0.next().map(HtmlNodeMut)
    }

    /// Replace the node's descendants with `text`, DOM `textContent=`.
    ///
    /// `false` when Lexbor could not store it, in which case the node keeps
    /// what it had.
    pub fn set_text_content(self, text: &[u8]) -> bool {
        // SAFETY: a live node the caller may change; Lexbor copies the bytes
        // into the document before anything else runs.
        let st =
            unsafe { lxb::lxb_dom_node_text_content_set(self.as_raw(), text.as_ptr(), text.len()) };
        st == lxb::lexbor_status_t_LXB_STATUS_OK
    }

    /// The node as an element cleared for editing, when it is one.
    #[inline]
    pub fn element_mut(self) -> Option<HtmlElementMut<'doc>> {
        self.0.element().map(HtmlElementMut)
    }

    /// Take the node out of its tree. The arena keeps it, so a Ruby wrapper
    /// that still points at it stays valid - Makiri detaches, never destroys.
    #[inline]
    pub fn detach(self) {
        // SAFETY: a live node of a document the caller may change.
        unsafe { lxb::lxb_dom_node_remove(self.as_raw()) };
    }

    /// Append `node` as the last child of `self`.
    #[inline]
    pub fn insert_child(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: two live nodes of a document the caller may change.
        unsafe { lxb::lxb_dom_node_insert_child(self.as_raw(), node.as_raw()) };
    }

    /// Put `node` immediately before `self`.
    #[inline]
    pub fn insert_before(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: as above.
        unsafe { lxb::lxb_dom_node_insert_before(self.as_raw(), node.as_raw()) };
    }

    /// Put `node` immediately after `self`.
    #[inline]
    pub fn insert_after(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: as above.
        unsafe { lxb::lxb_dom_node_insert_after(self.as_raw(), node.as_raw()) };
    }
}

/// A node being built, before anything links it into the destination tree.
///
/// Cross-document translation makes one node at a time and inserts it under a
/// node it made a moment earlier. Neither the frozen check nor the evaluation
/// check behind [`HtmlNodeMut`] applies, because no wrapper has seen these
/// nodes and no query can reach them yet.
///
/// Neither this type nor [`BuildingElement`] has a `Drop`, and that is the
/// contract rather than an omission: a translation that fails part-way leaves
/// the half-built subtree for the document's arena to reclaim wholesale, and a
/// `Drop` here would instead destroy nodes already linked under their parent.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BuildingNode<'doc>(HtmlNode<'doc>);

impl<'doc> BuildingNode<'doc> {
    /// # Safety
    /// `raw` must be null, or a live node that outlives `'doc` and belongs to a
    /// subtree still being built - nothing outside that subtree points at it.
    #[inline]
    pub unsafe fn from_raw(raw: *mut LxbNode) -> Option<Self> {
        HtmlNode::from_raw(raw).map(BuildingNode)
    }

    /// The node as an ordinary handle, for reading.
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }

    #[inline]
    pub fn as_raw(self) -> *mut LxbNode {
        self.0.as_raw()
    }

    /// Where this node's CHILDREN attach: a `<template>`'s content fragment,
    /// the node itself otherwise.
    #[inline]
    pub fn link_target(self) -> Self {
        match self.0.template_content() {
            Some(content) => BuildingNode(content),
            None => self,
        }
    }

    /// This node's `<template>` contents fragment, still part of what is being
    /// built. `None` when the node is not an HTML `<template>`, and equally
    /// when it is one Lexbor gave no contents fragment.
    #[inline]
    pub fn template_content(self) -> Option<Self> {
        self.0.template_content().map(BuildingNode)
    }

    /// The next node in a pre-order walk of `root`'s subtree, staying inside
    /// the subtree being built.
    #[inline]
    pub fn preorder_next(self, root: Self) -> Option<Self> {
        self.0.preorder_next(root.0).map(BuildingNode)
    }

    /// Link `child` in as the last child.
    #[inline]
    pub fn insert_child(self, child: Self) {
        // SAFETY: two live nodes of one document, both still being built.
        unsafe { lxb::lxb_dom_node_insert_child(self.as_raw(), child.as_raw()) };
    }

    /// Link `node` in immediately before this one, under the same parent.
    #[inline]
    pub fn insert_before(self, node: Self) {
        // SAFETY: as `insert_child`.
        unsafe { lxb::lxb_dom_node_insert_before(self.as_raw(), node.as_raw()) };
    }
}

/// An element being filled in before anything links it into a tree.
///
/// Cross-document translation creates an element, copies the source's
/// attributes onto it, and hands it back for the caller to insert. It is NOT
/// [`HtmlElementMut`]: that type means the receiver passed the frozen and
/// evaluation checks, which say nothing about an element this code just made.
///
/// Nor is it [`ScratchElement`], which destroys what it holds. A half-built
/// subtree that is abandoned on failure is left where it is: the document's
/// arena reclaims it wholesale, and nothing else ever points at it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BuildingElement<'doc>(HtmlElement<'doc>);

impl<'doc> BuildingElement<'doc> {
    /// # Safety
    /// `el` must be a live element that outlives `'doc`, freshly created and
    /// not yet linked into any tree.
    #[inline]
    pub unsafe fn from_raw(el: *mut LxbElement) -> Option<Self> {
        HtmlNode::from_raw(el as *mut LxbNode).map(|n| BuildingElement(HtmlElement(n)))
    }

    /// The element as a node being built, for linking and for reading.
    #[inline]
    pub fn as_node(self) -> BuildingNode<'doc> {
        BuildingNode(self.0.node())
    }

    /// Put the element in the interned namespace `ns_id`.
    #[inline]
    pub fn set_ns(self, ns_id: usize) {
        // SAFETY: an element nothing else holds; `ns_id` is an id this
        // document's own namespace table handed out.
        unsafe { (*self.0.raw()).node.ns = ns_id };
    }

    /// Set a plain, namespaceless attribute. `false` when Lexbor could not
    /// store it.
    pub fn set_attribute(self, name: &[u8], value: &[u8]) -> bool {
        // SAFETY: an element nothing else holds; both slices are read and
        // copied by Lexbor.
        let at = unsafe {
            lxb::lxb_dom_element_set_attribute(
                self.0.raw(),
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
            )
        };
        !at.is_null()
    }

    /// Create an attribute in `ns`, name it `qname` case-preserving, give it
    /// `value`, and append it. `false` when any step failed, in which case the
    /// unappended attribute is left for the arena, like the rest of an
    /// abandoned subtree.
    pub fn append_ns_attribute(self, ns: &[u8], qname: &[u8], value: &[u8]) -> bool {
        // SAFETY: an element nothing else holds, in a live document; every
        // slice is read and copied by Lexbor.
        unsafe {
            let doc = self.0.node().owner_document();
            let at = lxb::lxb_dom_attr_interface_create(doc);
            if at.is_null() {
                return false;
            }
            let named = lxb::lxb_dom_attr_set_name_ns(
                at,
                ns.as_ptr(),
                ns.len(),
                qname.as_ptr(),
                qname.len(),
                false,
            );
            if named != lxb::lexbor_status_t_LXB_STATUS_OK
                || lxb::lxb_dom_attr_set_value(at, value.as_ptr(), value.len())
                    != lxb::lexbor_status_t_LXB_STATUS_OK
            {
                return false;
            }
            lxb::lxb_dom_element_attr_append(self.0.raw(), at);
            true
        }
    }
}

/// An element made only to be read from and thrown away, destroyed when it
/// goes out of scope.
///
/// Renaming an element is done by creating one under the new name, copying the
/// names the document interned for it, and discarding the source: the five
/// fields copied out (`local_name`, `prefix`, `ns`, `upper_name`,
/// `qualified_name`) are the DOCUMENT's interned strings and tag ids, not the
/// element's own storage, so they outlive it.
///
/// This is the one place Makiri destroys rather than detaches, and it is sound
/// for the same reason: the throwaway was never in a tree and no Ruby wrapper
/// ever saw it. Owning it keeps the destroy off the success path, where it used
/// to sit between the copies and the index drop.
pub struct ScratchElement<'doc>(HtmlElement<'doc>);

impl<'doc> ScratchElement<'doc> {
    /// A detached element named `local_name`, or `None` when Lexbor could not
    /// make one.
    ///
    /// # Safety
    /// `doc` must be a live document that outlives `'doc`.
    pub unsafe fn create(doc: *mut LxbDoc, local_name: &[u8]) -> Option<Self> {
        let el = lxb::lxb_dom_document_create_element(
            doc,
            local_name.as_ptr(),
            local_name.len(),
            core::ptr::null_mut(),
        );
        HtmlNode::from_raw(el as *mut LxbNode).map(|n| ScratchElement(HtmlElement(n)))
    }

    /// Give `target` this element's interned name, in place, so a Ruby wrapper
    /// pointing at `target` keeps pointing at the same element.
    pub fn rename(&self, target: HtmlElementMut<'doc>) {
        // SAFETY: two live elements of one document; the names copied are the
        // document's interned storage, which outlives this scratch element.
        unsafe {
            let (to, from) = (target.element().raw(), self.0.raw());
            (*to).node.local_name = (*from).node.local_name;
            (*to).node.prefix = (*from).node.prefix;
            (*to).node.ns = (*from).node.ns;
            (*to).upper_name = (*from).upper_name;
            (*to).qualified_name = (*from).qualified_name;
        }
    }
}

impl Drop for ScratchElement<'_> {
    fn drop(&mut self) {
        // SAFETY: this type owns the element, which was never in a tree.
        unsafe { lxb::lxb_dom_node_destroy(self.0.node().as_raw()) };
    }
}

/// An element cleared for editing, reached through [`HtmlNodeMut::element_mut`].
///
/// Same clearance as the node it came from: the receiver was not frozen, and no
/// XPath evaluation is reading its document.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlElementMut<'doc>(HtmlElement<'doc>);

impl<'doc> HtmlElementMut<'doc> {
    /// The element as an ordinary handle, for reading.
    #[inline]
    pub fn element(self) -> HtmlElement<'doc> {
        self.0
    }

    /// Set `name` to `value`, adding the attribute when the element has none.
    ///
    /// `None` when Lexbor could not store it. The lookup is Lexbor's own, by
    /// local name and lower-cased for HTML - `set_attribute_ns` is the one that
    /// keys on (namespace, local name) instead.
    pub fn set_attribute(self, name: &[u8], value: &[u8]) -> Option<HtmlAttr<'doc>> {
        // SAFETY: a live element the caller may change; both slices are read
        // and copied by Lexbor before anything else runs.
        let at = unsafe {
            lxb::lxb_dom_element_set_attribute(
                self.0.raw(),
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
            )
        };
        HtmlNode::link(at as *mut LxbNode).map(HtmlAttr)
    }

    /// Create an attribute named `qname`, give it `value`, and append it.
    ///
    /// `ns` is the namespace URI, or `None` for none - which is a different
    /// naming call, not an empty URI, so the two cannot be folded together. A
    /// fresh attribute is calloc'd into the null namespace already, so only the
    /// namespaced setter has to say anything about it.
    ///
    /// `false` when any step failed; the un-appended attribute is left for the
    /// document's arena to reclaim wholesale, this module's "never destroy"
    /// convention.
    pub fn append_attribute(self, ns: Option<&[u8]>, qname: &[u8], value: &[u8]) -> bool {
        // SAFETY: a live element of a live document the caller may change;
        // every slice is read and copied by Lexbor.
        unsafe {
            let at = lxb::lxb_dom_attr_interface_create(self.0.node().owner_document());
            let Some(at) = HtmlNode::link(at as *mut LxbNode).map(HtmlAttr) else {
                return false;
            };
            let named = match ns {
                Some(uri) => lxb::lxb_dom_attr_set_name_ns(
                    at.raw(),
                    uri.as_ptr(),
                    uri.len(),
                    qname.as_ptr(),
                    qname.len(),
                    false,
                ),
                None => lxb::lxb_dom_attr_set_name(at.raw(), qname.as_ptr(), qname.len(), false),
            };
            if named != lxb::lexbor_status_t_LXB_STATUS_OK || !at.set_value(value) {
                return false;
            }
            lxb::lxb_dom_element_attr_append(self.0.raw(), at.raw());
            true
        }
    }

    /// Take `attr` off the element. The arena keeps it, like a detached node.
    pub fn attr_remove(self, attr: HtmlAttr<'doc>) {
        // SAFETY: a live element the caller may change, and an attribute of it.
        unsafe { lxb::lxb_dom_element_attr_remove(self.0.raw(), attr.raw()) };
    }

    /// Remove the attribute Lexbor's lookup finds for `name`; no-op when there
    /// is none.
    pub fn remove_attribute(self, name: &[u8]) {
        // SAFETY: as above.
        unsafe {
            lxb::lxb_dom_element_remove_attribute(self.0.raw(), name.as_ptr(), name.len());
        }
    }
}

impl<'doc> HtmlElement<'doc> {
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }
    #[inline]
    fn raw(self) -> *mut LxbElement {
        self.0.as_raw() as *mut LxbElement
    }

    pub fn qualified_name(self) -> &'doc [u8] {
        // SAFETY: a live element.
        unsafe { named(self.raw(), lxb::lxb_dom_element_qualified_name) }
    }
    pub fn local_name(self) -> &'doc [u8] {
        // SAFETY: a live element.
        unsafe { named_mut(self.raw(), lxb::lxb_dom_element_local_name) }
    }
    /// DOM `tagName`, or None when Lexbor has none.
    pub fn tag_name(self) -> Option<&'doc [u8]> {
        let mut len = 0usize;
        // SAFETY: a live element.
        let p = unsafe { lxb::lxb_dom_element_tag_name(self.raw(), &mut len) };
        // SAFETY: Lexbor's interned name, `len` bytes.
        (!p.is_null()).then(|| unsafe { seen(p, len) })
    }

    /// The first attribute, read straight from the element.
    #[inline]
    pub fn first_attr(self) -> Option<HtmlAttr<'doc>> {
        // SAFETY: a live element; its attribute list belongs to the document.
        HtmlNode::link(unsafe { first_attr(self.0.as_raw()) }).map(HtmlAttr)
    }

    /// The attributes, in document order.
    pub fn attrs(self) -> Attrs<'doc> {
        // SAFETY: a live element.
        let first = unsafe { lxb::lxb_dom_element_first_attribute_noi(self.raw()) };
        Attrs(HtmlNode::link(first as *mut LxbNode).map(HtmlAttr))
    }

    /// The attribute with this (namespace, local name) - the DOM's key for a
    /// namespaced attribute, as against [`get_attribute`](Self::get_attribute),
    /// which is Lexbor's lookup by local name alone.
    ///
    /// The local name compared is the CASE-PRESERVED tail of the qualified
    /// name, not Lexbor's stored `local_name`: Lexbor lower-cases that even when
    /// the qualified name keeps its case, and `setAttributeNS` is
    /// case-sensitive.
    pub fn find_attr_ns(self, ns_id: usize, local: &[u8]) -> Option<HtmlAttr<'doc>> {
        let mut at = self.first_attr();
        while let Some(a) = at {
            if a.own_ns() == ns_id {
                let q = a.qualified_name();
                let l = a.local_name();
                if q.len() >= l.len() && &q[q.len() - l.len()..] == local {
                    return Some(a);
                }
            }
            at = a.next_attr();
        }
        None
    }

    /// Lexbor's attribute lookup (by local name, lower-cased for HTML).
    pub fn has_attribute(self, name: &[u8]) -> bool {
        // SAFETY: a live element; `name` is only read.
        unsafe { lxb::lxb_dom_element_has_attribute(self.raw(), name.as_ptr(), name.len()) }
    }
    /// The value [`has_attribute`](Self::has_attribute) finds, or None.
    pub fn get_attribute(self, name: &[u8]) -> Option<&'doc [u8]> {
        // SAFETY: a live element; `name` is only read.
        unsafe { get_attribute(self.0.as_raw(), name) }
    }
}

impl<'doc> HtmlAttr<'doc> {
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }
    #[inline]
    pub(crate) fn raw(self) -> *mut LxbAttr {
        self.0.as_raw() as *mut LxbAttr
    }

    /// The next attribute of the same element, read straight from this one.
    #[inline]
    pub fn next_attr(self) -> Option<HtmlAttr<'doc>> {
        // SAFETY: a live attribute; the next one is in the same list.
        HtmlNode::link(unsafe { attr_next(self.0.as_raw()) }).map(HtmlAttr)
    }

    #[inline]
    pub fn qualified_name(self) -> &'doc [u8] {
        // SAFETY: a live attribute.
        unsafe { named(self.raw(), lxb::lxb_dom_attr_qualified_name) }
    }
    #[inline]
    pub fn local_name(self) -> &'doc [u8] {
        // SAFETY: a live attribute.
        unsafe { named(self.raw(), lxb::lxb_dom_attr_local_name) }
    }
    #[inline]
    pub fn value(self) -> &'doc [u8] {
        // SAFETY: a live attribute; the value is only changed by a mutator,
        // which the handle's contract rules out for 'doc.
        unsafe { named_mut(self.raw(), lxb::lxb_dom_attr_value_noi) }
    }
    /// Replace the attribute's value. `false` when Lexbor could not store it,
    /// in which case the attribute keeps what it had.
    ///
    /// Lexbor frees the old value here, which is why an XPath evaluation may not
    /// be reading this document - the borrowed slices it holds would dangle.
    /// Reaching this through [`HtmlElementMut`] is what says that was checked.
    pub fn set_value(self, value: &[u8]) -> bool {
        // SAFETY: a live attribute; Lexbor copies the bytes before anything
        // else runs.
        let st = unsafe { lxb::lxb_dom_attr_set_value(self.raw(), value.as_ptr(), value.len()) };
        st == lxb::lexbor_status_t_LXB_STATUS_OK
    }

    /// The attribute's OWN namespace id, the one `setAttributeNS` recorded.
    ///
    /// An attribute with no namespace of its own reports its element's, so the
    /// two are compared: only a difference is the attribute's own. [`NS_UNDEF`]
    /// for an attribute Lexbor has not linked to an element yet.
    pub fn own_ns(self) -> usize {
        match self.owner() {
            Some(owner) if owner.node().ns_id() != self.node().ns_id() => self.node().ns_id(),
            _ => NS_UNDEF,
        }
    }

    /// The element the attribute is set on, when Lexbor has linked it.
    pub fn owner(self) -> Option<HtmlElement<'doc>> {
        // SAFETY: a live attribute.
        let owner = unsafe { (*self.raw()).owner };
        HtmlNode::link(owner as *mut LxbNode).map(HtmlElement)
    }
}

/// [`HtmlNode::children`].
pub struct Siblings<'doc>(Option<HtmlNode<'doc>>);

impl<'doc> Iterator for Siblings<'doc> {
    type Item = HtmlNode<'doc>;
    #[inline]
    fn next(&mut self) -> Option<HtmlNode<'doc>> {
        let n = self.0?;
        self.0 = n.next();
        Some(n)
    }
}

/// [`HtmlNode::ancestors`].
pub struct Ancestors<'doc>(Option<HtmlNode<'doc>>);

impl<'doc> Iterator for Ancestors<'doc> {
    type Item = HtmlNode<'doc>;
    #[inline]
    fn next(&mut self) -> Option<HtmlNode<'doc>> {
        let n = self.0?;
        self.0 = n.parent();
        Some(n)
    }
}

/// [`HtmlElement::attrs`].
pub struct Attrs<'doc>(Option<HtmlAttr<'doc>>);

impl<'doc> Iterator for Attrs<'doc> {
    type Item = HtmlAttr<'doc>;
    #[inline]
    fn next(&mut self) -> Option<HtmlAttr<'doc>> {
        let a = self.0?;
        // SAFETY: a live attribute.
        let next = unsafe { lxb::lxb_dom_element_next_attribute_noi(a.raw()) };
        self.0 = HtmlNode::link(next as *mut LxbNode).map(HtmlAttr);
        Some(a)
    }
}
