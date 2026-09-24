//! Making nodes: the document's factories, and the handles for a node that is
//! being built and not yet in a tree ([`BuildingNode`], [`BuildingElement`]).

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use super::*;

impl<'doc> HtmlDoc<'doc> {
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

    /// A detached element named `local` in `ns`, with `prefix` when it is not
    /// empty - the DOM's createElementNS, where [`create_element`] is
    /// createElement: that takes `p:e` as one local name, and lower-cases it.
    /// Here the name keeps its case (`linearGradient`), as the parser keeps a
    /// foreign element's: the lower-cased tag is Lexbor's key, and the name as
    /// written is recorded beside it. An empty `ns` is no namespace. `None`
    /// when Lexbor could not make one.
    ///
    /// [`create_element`]: Self::create_element
    pub fn create_element_ns(
        self,
        local: &[u8],
        ns: &[u8],
        prefix: &[u8],
    ) -> Option<BuildingElement<'doc>> {
        let or_null = |s: &[u8]| {
            if s.is_empty() {
                core::ptr::null()
            } else {
                s.as_ptr()
            }
        };
        // SAFETY: a live document; Lexbor copies every name into its own
        // storage, and a failure destroys the half-made element itself. A null
        // prefix is "none" - a non-null empty one would be interned as a
        // prefix of its own.
        let el = unsafe {
            BuildingElement::from_raw(lxb::lxb_dom_element_create(
                self.as_raw(),
                local.as_ptr(),
                local.len(),
                or_null(ns),
                ns.len(),
                or_null(prefix),
                prefix.len(),
                core::ptr::null(),
                0,
                false,
            ))
        }?;
        /* With a prefix, Lexbor recorded `prefix:local` as written already. */
        if prefix.is_empty() && local.iter().any(u8::is_ascii_uppercase) {
            // SAFETY: an element just made in this document, in no tree; the
            // name is copied.
            let st = unsafe {
                lxb::lxb_dom_element_qualified_name_set(
                    el.0.raw(),
                    core::ptr::null(),
                    0,
                    local.as_ptr(),
                    local.len(),
                )
            };
            if st != lxb::consts::STATUS_OK {
                return None;
            }
        }
        Some(el)
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
    /// that up (see `lexbor::fragment::import_with_fixup`).
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

    /// Forget the source position of this node and everything below it.
    /// (A `<template>`'s contents are never stamped - the position walk goes
    /// through children - so there is nothing there to forget.)
    ///
    /// For a copy made from ANOTHER document: Lexbor's import copies `user`,
    /// and the offset in it indexes the source document's text, so `#line`
    /// answered with a line of this document the node was never on (41 in the
    /// source became 22 here). No position is the truthful answer - nil.
    /// Iterative, like every walk over a tree built from input.
    pub fn clear_source_offsets(self) {
        let mut cur = Some(self);
        while let Some(n) = cur {
            n.0.forget_source_offset();
            cur = n.preorder_next(self);
        }
    }

    /// Give this copy of `src` the name `src` records as written, which
    /// Lexbor's copy leaves behind (`lxb_dom_element_interface_copy` copies the
    /// tag, not the spelling): a copied SVG `linearGradient` read
    /// `lineargradient`, and `p:Bar` read `bar`. Nothing to do for a node
    /// with no written name. `false` when Lexbor could not store it.
    pub fn copy_written_name_from(self, src: HtmlNode<'_>) -> bool {
        let (Some(from), Some(to)) = (src.element(), self.0.element()) else {
            return true;
        };
        if !from.has_written_name() {
            return true;
        }
        let name = from.qualified_name();
        // SAFETY: an element being built, in no tree yet; the name is copied
        // into this document's tag table.
        let st = unsafe {
            lxb::lxb_dom_element_qualified_name_set(
                to.raw(),
                core::ptr::null(),
                0,
                name.as_ptr(),
                name.len(),
            )
        };
        st == lxb::consts::STATUS_OK
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
/// A half-built subtree that is abandoned on failure is left where it is: the
/// document's arena reclaims it wholesale, and nothing else ever points at it.
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
        self.0.put_attribute(name, value).is_some()
    }

    /// Create an attribute in `ns`, name it `qname` case-preserving, give it
    /// `value`, and append it. `false` when any step failed, in which case the
    /// unappended attribute is left for the arena, like the rest of an
    /// abandoned subtree.
    pub fn append_ns_attribute(self, ns: &[u8], qname: &[u8], value: &[u8]) -> bool {
        self.0.append_attribute_ns(Some(ns), qname, value)
    }
}
