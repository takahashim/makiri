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
    /// An HTML-namespace name with upper case is the exception: its lower-cased
    /// tag would make it that tag's element (`BR` the void `br`), so it takes
    /// a tag of its own ([`create_html_element_as_written`]).
    ///
    /// [`create_element`]: Self::create_element
    /// [`create_html_element_as_written`]: Self::create_html_element_as_written
    pub fn create_element_ns(
        self,
        local: &[u8],
        ns: &[u8],
        prefix: &[u8],
    ) -> Option<BuildingElement<'doc>> {
        if has_ascii_uppercase(local) && self.lookup_ns(ns) == Some(NsId::HTML) {
            return self.create_html_element_as_written(local, prefix);
        }
        /* The namespace is interned here, as written, and only a built-in
         * one is named to Lexbor - by its exact URI, which its case-folding
         * lookup maps back to the same id. Any other is created in no
         * namespace and given its id after: handed to Lexbor, `fooNamespace`
         * would be interned lower-cased, and `HTTP://WWW.W3.ORG/1999/XHTML`
         * would make an HTML element. In no namespace and in a namespace past
         * the built-in ones, Lexbor builds the same plain element struct. */
        let ns_id = if ns.is_empty() {
            None
        } else {
            Some(self.intern_ns(ns)?)
        };
        let lexbor_ns: &[u8] = match ns_id {
            Some(id) if id.is_static() => ns,
            _ => &[],
        };
        let or_null = |s: &[u8]| {
            if s.is_empty() {
                core::ptr::null()
            } else {
                s.as_ptr()
            }
        };
        /* The prefix steps are taken here, in Lexbor's order, rather than by
         * its create (hardening): without a prefix nothing can fail once the
         * element exists, and a failure among these leaves the element to the
         * arena, as every abandoned build here does, rather than destroying
         * it. */
        // SAFETY: a live document; Lexbor copies every name into its own
        // storage.
        let el = unsafe {
            BuildingElement::from_raw(lxb::lxb_dom_element_create(
                self.as_raw(),
                local.as_ptr(),
                local.len(),
                or_null(lexbor_ns),
                lexbor_ns.len(),
                core::ptr::null(),
                0,
                core::ptr::null(),
                0,
                false,
            ))
        }?;
        if let Some(id) = ns_id.filter(|id| !id.is_static()) {
            // SAFETY: an element just made in this document, in no tree; `id`
            // is interned in its namespace table.
            unsafe { (*el.0.raw()).node.ns = id.raw() };
        }
        let (p, p_len) = if prefix.is_empty() {
            if !has_ascii_uppercase(local) {
                return Some(el);
            }
            (core::ptr::null(), 0)
        } else {
            // SAFETY: a live document; the prefix is copied into its table.
            let data = unsafe {
                lxb::lxb_ns_prefix_append((*self.as_raw()).prefix, prefix.as_ptr(), prefix.len())
            };
            if data.is_null() {
                return None;
            }
            // SAFETY: an element just made in this document, in no tree, and a
            // live entry of the document's prefix table.
            unsafe { (*el.0.raw()).node.prefix = (*data).prefix_id };
            (prefix.as_ptr(), prefix.len())
        };
        /* The name as written beside the lower-cased tag: `prefix:local`, or
         * an unprefixed `local` with upper case. */
        // SAFETY: an element just made in this document, in no tree; the names
        // are copied.
        let st = unsafe {
            lxb::lxb_dom_element_qualified_name_set(
                el.0.raw(),
                p,
                p_len,
                local.as_ptr(),
                local.len(),
            )
        };
        if st != lxb::consts::STATUS_OK {
            return None;
        }
        Some(el)
    }

    /// An HTML-namespace element named `local` as written, with `prefix` when
    /// it is not empty - for a `local` with upper case, which the DOM's
    /// createElementNS makes an unknown element of that name (the HTML
    /// Standard's "element interface" and "serializes as void" compare names
    /// case-sensitively). `None` when Lexbor could not make one.
    ///
    /// `lxb_dom_element_create` cannot: it takes the tag from the lower-cased
    /// name, and the struct and serialization from the tag, so `BR` was made
    /// the void `br` (a child appended to it vanished from `to_html`) and
    /// `SCRIPT` a raw-text element. Here the tag is interned as written
    /// (`lxb_tag_append`), which gives `BR` a tag of its own, past Lexbor's
    /// static range - and Lexbor makes an HTML element with such a tag an
    /// HTMLUnknownElement, copies it by that name, and never matches it by
    /// `br`. The rest is `lxb_dom_element_create`'s own steps, in its order.
    fn create_html_element_as_written(
        self,
        local: &[u8],
        prefix: &[u8],
    ) -> Option<BuildingElement<'doc>> {
        if local.is_empty() {
            return None;
        }
        // SAFETY: a live document; Lexbor copies every name into its own
        // tables (none is empty, so no hash reads past one). The element is
        // fresh, in no tree, and destroyed here if a later step fails.
        unsafe {
            let doc = self.as_raw();
            let tag = lxb::lxb_tag_append(
                (*doc).tags,
                lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as lxb::lxb_tag_id_t,
                local.as_ptr(),
                local.len(),
            );
            if tag.is_null() {
                return None;
            }
            let el =
                lxb::lxb_dom_document_create_interface_noi(doc, (*tag).tag_id, NsId::HTML.raw())
                    as *mut LxbElement;
            if el.is_null() {
                return None;
            }
            let abandon = |el: *mut LxbElement| {
                lxb::lxb_dom_document_destroy_interface_noi(el as *mut core::ffi::c_void);
                None
            };
            let (p, p_len) = if prefix.is_empty() {
                (core::ptr::null(), 0)
            } else {
                let data = lxb::lxb_ns_prefix_append((*doc).prefix, prefix.as_ptr(), prefix.len());
                if data.is_null() {
                    return abandon(el);
                }
                (*el).node.prefix = (*data).prefix_id;
                (prefix.as_ptr(), prefix.len())
            };
            /* The written name beside the tag, as the other paths record one:
             * the readers take the DOM's name from it. */
            if lxb::lxb_dom_element_qualified_name_set(el, p, p_len, local.as_ptr(), local.len())
                != lxb::consts::STATUS_OK
            {
                return abandon(el);
            }
            (*el).custom_state =
                lxb::lxb_dom_element_custom_state_t_LXB_DOM_ELEMENT_CUSTOM_STATE_UNCUSTOMIZED;
            BuildingElement::from_raw(el)
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
    /// one - including data holding `?>`, which Lexbor refuses. The TARGET it
    /// does not check (a TODO in Lexbor), so the caller validates it first.
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

            /* An id whose empty string could not be allocated keeps its NULL
             * `data`, which is the unimportable doctype described above: fail
             * closed, leaving the doctype to the arena as above. */
            let text = (*self.as_raw()).text;
            for id in [&mut (*dt).public_id, &mut (*dt).system_id] {
                if id.data.is_null() && lxb::lexbor_str_init(id, text, 0).is_null() {
                    return None;
                }
            }
            BuildingNode::from_raw(dt as *mut LxbNode)
        }
    }

    /// Whether `name` satisfies the DOM's doctype-name production, which
    /// [`create_doctype`](Self::create_doctype) requires of its caller.
    ///
    /// [`crate::xml::dom_name::valid_doctype_name`], the rule the XML factory
    /// applies too, so the two answer alike from one definition. It is byte
    /// for byte `lxb_dom_document_type_valid_name` - whitespace, NUL and `>`
    /// refused, and the empty name, which Lexbor reads as absent - which this
    /// once called through FFI (and which Lexbor's create still applies).
    pub fn valid_doctype_name(name: &[u8]) -> bool {
        crate::xml::dom_name::valid_doctype_name(name)
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
    ///
    /// Lexbor's copy appends attributes by its own rules, which can drop one;
    /// `attrs::repair_import` puts the copy's attributes right before it is
    /// handed out, and a copy it cannot repair is `None` too.
    ///
    /// An attribute and a document fragment are copied by steps of their
    /// own (hardening): an attribute by Lexbor's attribute clone, a fragment
    /// made afresh and given copies of the children. Lexbor's importNode is
    /// given every other kind.
    pub fn import_node(self, src: HtmlNode<'_>, deep: bool) -> Option<BuildingNode<'doc>> {
        match src.node_type() {
            NodeType::Attribute => {
                // SAFETY: a live attribute, only read; the clone is made in
                // this document, unlinked and owned by no element.
                let copy = unsafe {
                    lxb::lxb_dom_attr_interface_clone(self.as_raw(), src.as_raw() as *mut LxbAttr)
                };
                // SAFETY: Lexbor's attribute begins with its node.
                return unsafe { BuildingNode::from_raw(copy as *mut LxbNode) };
            }
            NodeType::DocumentFragment => {
                let copy = self.create_fragment()?;
                if deep {
                    /* A child is never a fragment, so this goes one level. */
                    for child in src.children() {
                        copy.insert_child(self.import_node(child, true)?);
                    }
                }
                return Some(copy);
            }
            _ => {}
        }
        // SAFETY: two live documents' nodes; Lexbor allocates the copy in this
        // one and leaves the source alone. The copy is unshared until
        // returned, which is what the repair asks.
        unsafe {
            let copy = BuildingNode::from_raw(lxb::lxb_dom_document_import_node(
                self.as_raw(),
                src.as_raw(),
                deep,
            ))?;
            super::attrs::repair_import(self, src, copy.0, deep).ok()?;
            Some(copy)
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
    pub(in crate::lexbor::adapter) unsafe fn from_raw(raw: *mut LxbNode) -> Option<Self> {
        HtmlNode::from_raw(raw).map(BuildingNode)
    }

    /// `node` as a handle for building under.
    ///
    /// # Safety
    /// As [`RawNode::as_node`], and `node` belongs to a subtree still being
    /// built - nothing outside that subtree points at it.
    #[inline]
    pub unsafe fn from_raw_node(node: RawNode) -> Self {
        BuildingNode(node.as_node())
    }

    /// The node as an ordinary handle, for reading.
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }

    #[inline]
    pub(in crate::lexbor::adapter) fn as_raw(self) -> *mut LxbNode {
        self.0.as_raw()
    }

    /// Forget the source position of this node and everything below it,
    /// `<template>` contents included: those are stamped at creation like any
    /// other element, and a deep copy carries them along.
    ///
    /// For a copy made from ANOTHER document: Lexbor's import copies `user`,
    /// and the offset in it indexes the source document's text, so `#line`
    /// answered with a line of this document the node was never on (41 in the
    /// source became 22 here). No position is the truthful answer - nil.
    /// Iterative, like every walk over a tree built from input.
    pub fn clear_source_offsets(self) {
        for n in self.0.subtree_with_contents() {
            n.forget_source_offset();
        }
    }

    /// Give this copy of `src` the name `src` records as written, which
    /// Lexbor's copy leaves behind (`lxb_dom_element_interface_copy` copies the
    /// tag, not the spelling): a copied SVG `linearGradient` read
    /// `lineargradient`, and `p:Bar` read `bar`. Nothing to do for a node
    /// with no written name. `Err` when Lexbor could not store it.
    pub fn copy_written_name_from(self, src: HtmlNode<'_>) -> Result<(), AdapterOom> {
        let (Some(from), Some(to)) = (src.element(), self.0.element()) else {
            return Ok(());
        };
        if !from.has_written_name() {
            return Ok(());
        }
        if src.owner_document() == self.0.owner_document() {
            /* One document, one tag table: the entry the source points at is
             * already this document's, so it is shared rather than looked up
             * again. Looking it up cost every copied SVG element a hash probe
             * (clone 10% slower, many small dups 20%) - and appending a name
             * the table already had can re-point its entry (see below). */
            // SAFETY: two live elements of one document; the tag entry is the
            // document's and outlives both.
            unsafe { (*to.raw()).qualified_name = (*from.raw()).qualified_name };
            return Ok(());
        }
        /* Another document's entry means nothing here, so the name is interned
         * in this one. A known gap, in Lexbor: `lxb_tag_append` given a name
         * the table already holds under ANOTHER tag - a parsed `<x:y>`, one
         * local name - re-points that entry, so the parsed element stops
         * matching CSS `x\:y`. It takes a prefixed `x:y` imported from another
         * document into one that already has a parsed `x:y`; see
         * NOKOGIRI_DIFFERENCES.md. */
        let name = from.qualified_name();
        // SAFETY: an element being built, in no tree yet; the name is copied
        // into this document's tag table.
        lexbor_ok(unsafe {
            lxb::lxb_dom_element_qualified_name_set(
                to.raw(),
                core::ptr::null(),
                0,
                name.as_ptr(),
                name.len(),
            )
        })
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

    /// The next node in a walk of `root`'s subtree that also enters every
    /// `<template>`'s contents; see [`HtmlNode::preorder_next_with_contents`].
    #[inline]
    pub fn preorder_next_with_contents(self, root: Self) -> Option<Self> {
        self.0.preorder_next_with_contents(root.0).map(BuildingNode)
    }

    /// Put every element of this subtree - template contents included - that
    /// is in no namespace into `ns`, an id of this node's document.
    ///
    /// For a fragment parsed with no namespace in place of its context's (see
    /// `lexbor::fragment::FragmentTag::parse_ns`): the parser makes no
    /// element in no namespace except by inheriting the context's, so these
    /// are exactly the elements that inherited it. The struct Lexbor chose for
    /// each is the one it chooses for any namespace outside its built-in ones.
    pub fn give_namespace(self, ns: NsId) {
        let mut next = Some(self);
        while let Some(n) = next {
            if n.0.node_type() == NodeType::Element && n.0.ns_id().is_none() {
                // SAFETY: an element still being built, which nothing else
                // refers to; `ns` is interned in its document.
                unsafe { (*n.0.as_raw()).ns = ns.raw() };
            }
            next = n.preorder_next_with_contents(self);
        }
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
    pub(in crate::lexbor::adapter) unsafe fn from_raw(el: *mut LxbElement) -> Option<Self> {
        HtmlNode::from_raw(el as *mut LxbNode).map(|n| BuildingElement(HtmlElement(n)))
    }

    /// The element as a node being built, for linking and for reading.
    #[inline]
    pub fn as_node(self) -> BuildingNode<'doc> {
        BuildingNode(self.0.node())
    }

    /// Put the element in the interned namespace `ns`.
    #[inline]
    pub fn set_ns(self, ns: NsId) {
        // SAFETY: an element nothing else holds; `ns` is an id this
        // document's own namespace table handed out.
        unsafe { (*self.0.raw()).node.ns = ns.raw() };
    }

    /// DOM "append an attribute" of a new one named `qname` (case preserved)
    /// in `ns`, `None` for no namespace - a copy's step, so no existing
    /// attribute is looked for. `Err` when any step failed, in which case the
    /// unappended attribute is left for the arena, like the rest of an
    /// abandoned subtree.
    pub fn append_attribute(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        // SAFETY: an element nothing else holds may be changed.
        unsafe { self.0.append_attribute(ns, qname, value) }
    }
}
