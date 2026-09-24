//! The one place Makiri reads Lexbor's DOM - node, element, attribute and
//! document fields, and the Lexbor accessors over them - and the one place it
//! edits the tree. An edit needs one of three clearance types, each with its own
//! contract: [`HtmlNodeMut`] / [`HtmlElementMut`] for a caller's node that
//! passed the frozen and evaluation checks (`mutate`), [`BuildingNode`] /
//! [`BuildingElement`] for a node no tree holds yet, and [`ScratchElement`] for
//! one made only to be read and destroyed (`build`).
//!
//! Everything here reads the GENERATED layout (`crate::lexbor::abi`), so there is
//! no hand-written copy of a Lexbor struct left to drift from the pinned headers.
//! The XPath engine's HTML backend and the Ruby-facing readers both come through
//! this module, so a field is read one way, in one place.
//!
//! Everything is reached through typed handles, whose contract is stated once:
//! the node is live, and the document it belongs to is not being changed while
//! a borrowed slice is in use - which `bridge::wrapper::DocumentEvaluation`
//! enforces for the one place Ruby can run mid-read, an XPath handler.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::lexbor::abi::{self as lxb, LxbAttr, LxbDoc, LxbElement, LxbNode};

mod build;
mod mutate;
pub use build::{BuildingElement, BuildingNode, ScratchElement};
pub use mutate::{HtmlElementMut, HtmlNodeMut, Insertion, Place, PreInsertError};

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
/// `LXB_NS_XMLNS`: the parser puts a foreign element's `xmlns` / `xmlns:*`
/// declarations in it, and XPath does not see them as attributes.
pub const NS_XMLNS: usize = lxb::lxb_ns_id_enum_t_LXB_NS_XMLNS as usize;

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

/* ---------- borrowed bytes ---------- */

/// A DOM `localName` from a qualified name and Lexbor's stored local name: the
/// qualified name's tail, at the stored name's length, so its case is kept.
fn case_preserved_tail<'a>(qualified: &'a [u8], local: &'a [u8]) -> &'a [u8] {
    if qualified.len() >= local.len() {
        &qualified[qualified.len() - local.len()..]
    } else {
        local
    }
}

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

/* ------------------------------------------------------------------ *
 * typed handles                                                      *
 * ------------------------------------------------------------------ */

/* The handles below state their contract once: holding one IS the proof that the node is live
 * and that its document is neither freed nor restructured while `'doc` lasts.
 * `HtmlNode::from_raw` is the only way to make one without already holding
 * one, so it is the single place that contract is asserted, and every method
 * is safe.
 *
 * "Not restructured" admits one kind of write, not to the tree's links: the
 * source location in `node.user` - stamped (`HtmlNode::stamp_source_offset`)
 * and, for a copy from another document, forgotten
 * (`HtmlNode::forget_source_offset`). That is why the
 * methods read fields through the raw pointer, place by place, rather than
 * holding a `&LxbNode` - no reference to a Lexbor struct outlives the read. */

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

/// A node pointer crossing the Ruby-glue boundary.
///
/// The glue holds and passes nodes as this. The field is private, so no module
/// outside `lexbor` names `lxb_dom_node_t` or reads its layout; where a field
/// must be read, [`as_node`](RawNode::as_node) lends the typed [`HtmlNode`].
/// It carries no `'doc`, because the Ruby wrapper owning the pointer is what
/// keeps the document alive, not a Rust borrow.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct RawNode(NonNull<LxbNode>);

impl RawNode {
    /// `None` for null.
    #[inline]
    pub fn from_ptr(p: *mut core::ffi::c_void) -> Option<Self> {
        NonNull::new(p as *mut LxbNode).map(RawNode)
    }

    /// The pointer, for storing in a Ruby wrapper's TypedData.
    #[inline]
    pub fn as_ptr(self) -> *mut core::ffi::c_void {
        self.0.as_ptr().cast()
    }

    /// The typed node pointer, for the adapter's own tables (the text index
    /// keys on it). Outside `lexbor`, nodes cross as `RawNode` or `c_void`.
    #[inline]
    pub(in crate::lexbor) fn as_lxb(self) -> *const LxbNode {
        self.0.as_ptr()
    }

    /// The typed node, lent for `'doc`.
    ///
    /// # Safety
    /// The node must be live and its document must outlive `'doc` without being
    /// restructured while `'doc` lasts - the [`HtmlNode`] contract.
    #[inline]
    pub unsafe fn as_node<'doc>(self) -> HtmlNode<'doc> {
        HtmlNode::from_raw(self.0.as_ptr()).expect("non-null by construction")
    }
}

impl<'doc> From<HtmlNode<'doc>> for RawNode {
    #[inline]
    fn from(n: HtmlNode<'doc>) -> Self {
        RawNode(n.raw)
    }
}

impl From<RawDoc> for RawNode {
    /// A document seen as its node: an `lxb_html_document_t` leads with its
    /// `lxb_dom_document_t`, which leads with its node.
    #[inline]
    fn from(d: RawDoc) -> Self {
        RawNode(d.0.cast())
    }
}

impl<'doc> From<BuildingNode<'doc>> for RawNode {
    #[inline]
    fn from(n: BuildingNode<'doc>) -> Self {
        RawNode::from(n.node())
    }
}

impl<'doc> From<BuildingElement<'doc>> for RawNode {
    #[inline]
    fn from(e: BuildingElement<'doc>) -> Self {
        RawNode::from(e.as_node())
    }
}

/// A document pointer crossing the Ruby-glue boundary. See [`RawNode`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct RawDoc(NonNull<LxbDoc>);

impl RawDoc {
    /// `None` for null.
    #[inline]
    pub fn from_ptr(p: *mut core::ffi::c_void) -> Option<Self> {
        NonNull::new(p as *mut LxbDoc).map(RawDoc)
    }

    /// The pointer, for storing in a Ruby wrapper's TypedData.
    #[inline]
    pub fn as_ptr(self) -> *mut core::ffi::c_void {
        self.0.as_ptr().cast()
    }

    /// The typed document, lent for `'doc`.
    ///
    /// # Safety
    /// As [`RawNode::as_node`].
    #[inline]
    pub unsafe fn as_doc<'doc>(self) -> HtmlDoc<'doc> {
        HtmlDoc::from_raw(self.0.as_ptr()).expect("non-null by construction")
    }
}

impl<'doc> From<HtmlDoc<'doc>> for RawDoc {
    #[inline]
    fn from(d: HtmlDoc<'doc>) -> Self {
        RawDoc(d.raw)
    }
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

    /// The raw pointer, for `lexbor/` only.
    ///
    /// Hidden so that reading a Lexbor struct field stays inside this layer:
    /// a `pub` raw pointer is how a document field (`compat_mode`) came to be
    /// read from `bridge`. Callers above use a named accessor instead.
    #[inline]
    pub(in crate::lexbor) fn as_raw(self) -> *mut LxbDoc {
        self.raw.as_ptr()
    }

    /// Lexbor's quirks mode: 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the
    /// parser from the doctype.
    #[inline]
    pub fn compat_mode(self) -> i64 {
        // SAFETY: a live document handle, read for this call.
        unsafe { (*self.raw.as_ptr()).compat_mode as i64 }
    }

    /// The tag id Lexbor knows `name` by, or [`TAG_UNDEF`] for an unknown or
    /// empty name. Custom-element ids are pointer values past
    /// [`TAG_LAST_ENTRY`], which callers bucketing by id must allow for.
    pub fn tag_id(self, name: &[u8]) -> usize {
        // SAFETY: a live document handle, read for this call.
        let tags = unsafe { (*self.raw.as_ptr()).tags };
        if name.is_empty() || tags.is_null() {
            return TAG_UNDEF;
        }
        // SAFETY: `tags` is the document's own tag table, `name` a live slice.
        unsafe { lxb::lxb_tag_id_by_name_noi(tags, name.as_ptr(), name.len()) }
    }

    /// The document as a node: an `lxb_dom_document_t` leads with its node.
    #[inline]
    pub fn as_node(self) -> HtmlNode<'doc> {
        HtmlNode {
            raw: self.raw.cast(),
            _doc: PhantomData,
        }
    }

    /// The document's `<title>` text, or None when there is none.
    pub fn title(self) -> Option<&'doc [u8]> {
        /* SAFETY: a live HTML document - an `lxb_html_document_t` leads with
         * its `lxb_dom_document_t`, so this is the same address - and the bytes
         * are borrowed from it. */
        let t = unsafe {
            named_mut(
                self.as_raw() as *mut lxb::lxb_html_document_t,
                lxb::lxb_html_document_title,
            )
        };
        (!t.is_empty()).then_some(t)
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

    /// The parent: for an attribute, the element it is set on.
    ///
    /// Lexbor keeps an attribute's element in `attr->owner` - set when it is
    /// appended, cleared when it is removed - and leaves `node.parent` null.
    /// The owner is read live, so a removed attribute, one on a detached element
    /// and one in a fragment all answer truly; the former backfill of
    /// `node.parent` at index time answered whatever held when the index was
    /// last built (`..` of a detached element's attribute depended on whether
    /// anything had queried the document before the detach).
    #[inline]
    pub fn parent(self) -> Option<Self> {
        if let Some(a) = self.attr() {
            return a.owner().map(HtmlElement::node);
        }
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

    /// The qualified name: an element's own (prefix and case preserved), any
    /// other node's DOM `nodeName`.
    pub fn qualified_name(self) -> &'doc [u8] {
        match self.element() {
            Some(el) => el.qualified_name(),
            None => self.node_name(),
        }
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

    /// The source byte offset the parse stamped on this element, or None when
    /// it could not be placed.
    ///
    /// Stored in Lexbor's `node.user`, which is reserved for exactly this, as
    /// offset + 1 so that a genuine offset of 0 is distinguishable from unset.
    /// [`stamp_source_offset`](Self::stamp_source_offset) is the other half of
    /// that encoding; nothing else reads or writes the field.
    pub fn source_offset(self) -> Option<usize> {
        // SAFETY: as `node_type`.
        let user = unsafe { (*self.as_raw()).user };
        (!user.is_null()).then(|| user as usize - 1)
    }

    /// Record `offset` as the element's source position - see
    /// [`source_offset`](Self::source_offset). Only the source-location
    /// stamping calls it, while the tree is still the one the parser built.
    pub(crate) fn stamp_source_offset(self, offset: usize) {
        // SAFETY: a live node; `user` is not part of the tree's structure.
        unsafe { (*self.as_raw()).user = offset.wrapping_add(1) as *mut core::ffi::c_void };
    }

    /// Forget this node's source position, so [`source_offset`](Self::source_offset)
    /// answers None. The second writer of `user`, beside the stamping: see
    /// [`BuildingNode::clear_source_offsets`](super::build::BuildingNode::clear_source_offsets).
    pub(crate) fn forget_source_offset(self) {
        // SAFETY: a live node; `user` is not part of the tree's structure.
        unsafe { (*self.as_raw()).user = core::ptr::null_mut() };
    }

    /// Whether the node's name carries a namespace prefix - one a namespaced
    /// name was given (createElementNS, setAttributeNS, the parser's foreign
    /// attributes). A parsed `fb:like` has none: its colon is part of one
    /// local name.
    #[inline]
    pub fn has_prefix(self) -> bool {
        // SAFETY: as `node_type`.
        unsafe { (*self.as_raw()).prefix != 0 }
    }

    /// The interned tag id (`local_name`).
    #[inline]
    pub fn tag_id(self) -> usize {
        // SAFETY: as `node_type`.
        unsafe { (*self.as_raw()).local_name }
    }

    /// The namespace URI, or None when the node has none.
    pub fn ns_uri(self) -> Option<&'doc [u8]> {
        let ns = self.ns_id();
        if ns == NS_UNDEF {
            return None;
        }
        // SAFETY: a live node's document, whose namespace table interns the
        // URI for the document's lifetime.
        let uri = unsafe {
            let table = (*self.owner_document().as_raw()).ns;
            if table.is_null() {
                return None;
            }
            let mut len = 0;
            seen(lxb::lxb_ns_by_id(table, ns, &mut len), len)
        };
        (!uri.is_empty()).then_some(uri)
    }

    /// The document the node belongs to. A live node's owner is a live
    /// document, kept alive by the same thing that keeps the node.
    #[inline]
    pub fn owner_document(self) -> HtmlDoc<'doc> {
        // SAFETY: as `node_type`; the owner outlives its node.
        unsafe { HtmlDoc::from_raw((*self.as_raw()).owner_document) }
            .expect("a live node has an owner document")
    }

    /// Whether both nodes belong to the same document.
    pub fn same_document(self, other: HtmlNode<'_>) -> bool {
        self.owner_document().as_raw() == other.owner_document().as_raw()
    }

    /// Where `other` falls against `self` in document (pre-order) order, or
    /// None when the two are not ordered: an attribute on either side (an
    /// attribute is not in the `first_child`/`next` chain), different
    /// documents, or detached subtrees with no common root.
    pub fn document_order(self, other: HtmlNode<'doc>) -> Option<core::cmp::Ordering> {
        use core::cmp::Ordering::{Equal, Greater, Less};
        let (a, b) = (self, other);
        if a == b {
            return Some(Equal);
        }
        if a.attr().is_some() || b.attr().is_some() || !a.same_document(b) {
            return None;
        }

        let (da, db) = (a.ancestors().count(), b.ancestors().count());
        let (mut pa, mut pb) = (a, b);

        /* Raise the deeper node to the other's depth; landing ON the other makes
         * that other an ancestor, which comes first in pre-order. */
        if da > db {
            for _ in 0..(da - db) {
                pa = pa.parent()?;
            }
            if pa == b {
                return Some(Greater);
            }
        } else if db > da {
            for _ in 0..(db - da) {
                pb = pb.parent()?;
            }
            if pb == a {
                return Some(Less);
            }
        }

        /* Climb both until they share a parent (the lowest common ancestor). A
         * missing parent on either side means different trees, or two roots. */
        let parent = loop {
            let (qa, qb) = (pa.parent()?, pb.parent()?);
            if qa == qb {
                break qa;
            }
            pa = qa;
            pb = qb;
        };

        /* pa and pb are distinct siblings: earlier in the child list is first. A
         * wide parent makes this the hot loop of a sort, so it is written out. */
        let mut c = parent.first_child();
        while let Some(x) = c {
            if x == pa {
                return Some(Less);
            }
            if x == pb {
                return Some(Greater);
            }
            c = x.next();
        }
        None /* unreachable for a well-formed tree */
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

    /// A text or CDATA node's data - the subset of [`data`](Self::data) that
    /// is text content - or None for any other kind.
    pub fn char_data(self) -> Option<&'doc [u8]> {
        self.data()
            .filter(|_| matches!(self.node_type(), TYPE_TEXT | TYPE_CDATA))
    }

    /// The data of any CharacterData node - text, CDATA, comment or processing
    /// instruction (whose data follows its target) - or None for any other
    /// kind.
    pub fn data(self) -> Option<&'doc [u8]> {
        if !matches!(
            self.node_type(),
            TYPE_TEXT | TYPE_CDATA | TYPE_COMMENT | TYPE_PI
        ) {
            return None;
        }
        // SAFETY: a live CharacterData node, which Lexbor allocates as one (a
        // PI embeds its character data first).
        unsafe {
            let cd = self.as_raw() as *mut lxb::lxb_dom_character_data_t;
            Some(seen((*cd).data.data, (*cd).data.length))
        }
    }

    /// Whether this is an HTML `<template>`, whose children live in a separate
    /// contents fragment rather than under it.
    pub fn is_html_template(self) -> bool {
        self.node_type() == TYPE_ELEMENT && self.tag_id() == TAG_TEMPLATE && self.ns_id() == NS_HTML
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
        if !self.is_html_template() {
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

    /// The DOM's `localName`, case preserved - see [`HtmlAttr::dom_local_name`];
    /// an SVG `foreignObject` is stored as `foreignobject`.
    pub fn dom_local_name(self) -> &'doc [u8] {
        case_preserved_tail(self.qualified_name(), self.local_name())
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
        HtmlNode::link(unsafe { (*self.raw()).first_attr } as *mut LxbNode).map(HtmlAttr)
    }

    /// The attributes, in document order.
    #[inline]
    pub fn attrs(self) -> Attrs<'doc> {
        Attrs(self.first_attr())
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
        self.attrs()
            .find(|a| a.own_ns() == ns_id && a.dom_local_name() == local)
    }

    /* The attribute-writing steps, spelled once. Reached only through the two
     * clearance types - `HtmlElementMut` (a receiver cleared for editing) and
     * `BuildingElement` (an element no tree holds yet) - which differ in who
     * may call them, not in what Lexbor is asked to do. */

    /// Set `name` to `value`, adding the attribute when the element has none -
    /// Lexbor's lookup, by local name and lower-cased for HTML. `None` when
    /// Lexbor could not store it.
    fn put_attribute(self, name: &[u8], value: &[u8]) -> Option<HtmlAttr<'doc>> {
        /* An attribute the element already has gets its value here, not in
         * `lxb_dom_element_set_attribute`: when storing the value fails, that
         * DESTROYS the attribute while it is still in the element's list - a
         * dangling link in the tree, and a freed node under whatever Ruby
         * wrapper holds it. A failure here leaves the attribute as it was.
         * The lookup is the one set_attribute makes, so the same attribute is
         * found. */
        // SAFETY: a live element; `name` is only read.
        let found =
            unsafe { lxb::lxb_dom_element_attr_is_exist(self.raw(), name.as_ptr(), name.len()) };
        if let Some(at) = HtmlNode::link(found as *mut LxbNode).map(HtmlAttr) {
            return at.set_value(value).then_some(at);
        }
        /* Only the create path is left, where a failure destroys an attribute
         * nothing links to yet. */
        // SAFETY: a live element its caller may change; both slices are read
        // and copied by Lexbor before anything else runs.
        let at = unsafe {
            lxb::lxb_dom_element_set_attribute(
                self.raw(),
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
            )
        };
        HtmlNode::link(at as *mut LxbNode).map(HtmlAttr)
    }

    /// Create an attribute named `qname` (case preserved), give it `value`, and
    /// append it. `ns` is the namespace URI, or `None` for none - a different
    /// naming call, not an empty URI; a fresh attribute is already in the null
    /// namespace.
    ///
    /// `false` when any step failed; the unappended attribute is left for the
    /// document's arena to reclaim wholesale, the "never destroy" convention.
    fn append_attribute_ns(self, ns: Option<&[u8]>, qname: &[u8], value: &[u8]) -> bool {
        // SAFETY: a live element of a live document its caller may change;
        // every slice is read and copied by Lexbor.
        unsafe {
            let at = lxb::lxb_dom_attr_interface_create(self.node().owner_document().as_raw());
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
            if named != lxb::consts::STATUS_OK || !at.set_value(value) {
                return false;
            }
            lxb::lxb_dom_element_attr_append(self.raw(), at.raw());
            true
        }
    }

    /// Lexbor's attribute lookup (by local name, lower-cased for HTML).
    pub fn has_attribute(self, name: &[u8]) -> bool {
        // SAFETY: a live element; `name` is only read.
        unsafe { lxb::lxb_dom_element_has_attribute(self.raw(), name.as_ptr(), name.len()) }
    }
    /// The value [`has_attribute`](Self::has_attribute) finds, or None.
    pub fn get_attribute(self, name: &[u8]) -> Option<&'doc [u8]> {
        let mut len = 0;
        // SAFETY: a live element; `name` is only read.
        let value = unsafe {
            lxb::lxb_dom_element_get_attribute(self.raw(), name.as_ptr(), name.len(), &mut len)
        };
        // SAFETY: Lexbor's stored value, `len` bytes, owned by the document.
        (!value.is_null()).then(|| unsafe { seen(value, len) })
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
        HtmlNode::link(unsafe { (*self.raw()).next } as *mut LxbNode).map(HtmlAttr)
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
    /// The DOM's `localName`: the qualified name after its prefix, case
    /// preserved. Lexbor lower-cases its stored local name even where the
    /// qualified name keeps its case - an SVG `refX` is stored as `refx` - so
    /// the tail of the qualified name is taken, at the stored name's length.
    pub fn dom_local_name(self) -> &'doc [u8] {
        case_preserved_tail(self.qualified_name(), self.local_name())
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
        st == lxb::consts::STATUS_OK
    }

    /// The attribute's OWN namespace id, the one `setAttributeNS` (or the
    /// parser's foreign-attribute adjustment) recorded.
    ///
    /// An attribute with no namespace of its own reports its element's, so a
    /// namespace is the attribute's own when it differs from its element's - or
    /// when the attribute has a prefix, which only a namespaced name is given:
    /// `set_attribute_ns(SVG, "q:x")` on an SVG element is in SVG, while a
    /// parsed `q:x` there is one no-namespace name. An UNPREFIXED namespaced
    /// attribute in its element's own namespace reads as no namespace - Lexbor
    /// keeps nothing that tells the two apart. [`NS_UNDEF`] for an unprefixed
    /// attribute Lexbor has not linked to an element yet.
    pub fn own_ns(self) -> usize {
        let ns = self.node().ns_id();
        match self.owner() {
            _ if self.node().has_prefix() => ns,
            Some(owner) if owner.node().ns_id() != ns => ns,
            _ => NS_UNDEF,
        }
    }

    /// The attribute's OWN namespace URI - see [`own_ns`](Self::own_ns) - or
    /// None when it has none.
    pub fn own_ns_uri(self) -> Option<&'doc [u8]> {
        if self.own_ns() == NS_UNDEF {
            return None;
        }
        self.node().ns_uri()
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
        self.0 = a.next_attr();
        Some(a)
    }
}
