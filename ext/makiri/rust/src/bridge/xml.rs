//! The Ruby <-> XML-arena DOM seam (the XML counterpart of [`crate::bridge::html`]).
//!
//! A Ruby XML node is an arena [`NodeId`] behind a TypedData wrapper; turning a
//! `Value` into that id, and running the Ruby-free mutation primitives over the
//! arena, is the same kind of unsafe boundary the HTML side has - so it lives
//! here, under `bridge`, and the glue layer stays free of it.
//!
//! The rules themselves (name well-formedness, the XML character class,
//! namespace resolution, what may be a child of what) live in the Ruby-free
//! `crate::xml`; this layer coerces and verifies arguments, and maps the
//! resulting status to a Ruby exception.
//!
//! **Detach, never destroy.** A removed node is unlinked, not freed, so a live
//! Ruby wrapper for it stays valid; the arena owns the memory and outlives every
//! wrapper through the Document.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::{prelude::*, Error, Value};
use crate::bridge::ruby::makiri_error;

use crate::bridge::dom::{XmlRead, XmlWrite};
use crate::bridge::html::with_arg_node;
use crate::bridge::ruby::is_kind_of;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::*;
use crate::bridge::wrapper::{new_xml_document, node_repr, DocHandle, NodeRepr};
use crate::bridge::xml_decode::xml_decode_input;
use crate::child_index::{ChildList, Children, TokenTree};
use crate::html_dom::cross_import::cross_html_to_xml;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT, EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::init::{
    CLASS_XML_ATTR, CLASS_XML_CDATA_SECTION, CLASS_XML_COMMENT, CLASS_XML_DOCUMENT_FRAGMENT,
    CLASS_XML_DOCUMENT_TYPE, CLASS_XML_ELEMENT, CLASS_XML_NODE, CLASS_XML_PROCESSING_INSTRUCTION,
    CLASS_XML_TEXT,
};
use crate::node_type::NodeType as CrateKind;
use crate::xml::attr_key::AttrKey;
use crate::xml::model::{ArenaKind, Document as XmlDoc, MutError, NodeId, ParseError, ParseLimits};
use crate::xml::mutate::{clone_node, copy_node_from, import_subtree, remove as remove_node};
use crate::xml::qname::NsDeclError;
use crate::xml::tree;

/* ------------------------------------------------------------------ *
 * the XML node front door                                            *
 * ------------------------------------------------------------------ */

/// The `Makiri::XML::*` leaves, by node type.
static XML_NODE_CLASSES: NodeClasses = NodeClasses {
    node: &CLASS_XML_NODE,
    element: &CLASS_XML_ELEMENT,
    attr: &CLASS_XML_ATTR,
    text: &CLASS_XML_TEXT,
    comment: &CLASS_XML_COMMENT,
    cdata: &CLASS_XML_CDATA_SECTION,
    pi: &CLASS_XML_PROCESSING_INSTRUCTION,
    doctype: &CLASS_XML_DOCUMENT_TYPE,
    fragment: &CLASS_XML_DOCUMENT_FRAGMENT,
};

/// Wrap an arena node into its `Makiri::XML::*` leaf.
///
/// The DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper, so the arena has exactly one owner. The id resolves through
/// `document`'s arena; a caller with no node holds an `Option` and maps `None`
/// to nil itself.
pub fn wrap_xml_node(ruby: &Ruby, id: NodeId, document: Value) -> Result<Value, Error> {
    let ty = xml_read(&document)?.type_(id);
    wrap_xml_node_of_type(ruby, id, ty, document)
}

/// [`wrap_xml_node`] for a node that must be one of `document`'s: an
/// `InternalError` when the arena has no node by that id.
pub(in crate::bridge) fn wrap_xml_node_checked(
    ruby: &Ruby,
    id: NodeId,
    document: Value,
) -> Result<Value, Error> {
    /* A hit was checked when it was made, as `wrap_html_node_checked`'s. */
    if let Some(cached) = crate::bridge::wrapper::cached_node(document, &id)? {
        return Ok(cached);
    }
    let ty = xml_read(&document)?.type_(id);
    if ty.is_none() {
        return Err(crate::bridge::ruby::internal_error(
            "a stored node of another document",
        ));
    }
    wrap_xml_node_of_type(ruby, id, ty, document)
}

/// [`wrap_xml_node`] for a caller that read the node's type already, in the
/// read it found the node in - one read the fewer.
fn wrap_xml_node_of_type(
    ruby: &Ruby,
    id: NodeId,
    ty: Option<ArenaKind>,
    document: Value,
) -> Result<Value, Error> {
    if ty == Some(ArenaKind::Document) {
        return Ok(document);
    }
    let klass = XML_NODE_CLASSES.class_for(ruby, ty.map_or(CrateKind::Other, Into::into))?;

    crate::bridge::wrapper::wrap_cached(ruby, &XML_NODE_TYPE, klass, id, document)
}

impl crate::bridge::wrapper::NodeHandleSource for NodeId {
    fn index_and_stamp(&self) -> (u32, u32) {
        (self.index(), self.stamp())
    }

    fn into_handle(self, _document: Value) -> NodeHandle {
        NodeHandle::Xml(self)
    }
}

/// The arena for [`crate::child_index`]: a node's token is its `NodeId`'s,
/// which the arena never reuses. The links are the arena's `Tree` ones,
/// which `#children` follows (attributes are not on them).
impl TokenTree for XmlDoc {
    fn token(&self, n: NodeId) -> usize {
        n.to_token()
    }
    fn node_of(&self, token: usize) -> Option<NodeId> {
        NodeId::from_token(token)
    }
}

impl ChildListHost for XmlSelf {
    type Tree<'a> = XmlDoc;
    fn document(&self) -> Value {
        self.document
    }
    fn with_children<R>(
        &self,
        list: ChildList,
        f: impl for<'a> FnOnce(Children<'a, XmlDoc>) -> R,
    ) -> Result<R, Error> {
        let view = self.read()?;
        Ok(f(Children {
            tree: &view,
            parent: self.id,
            list,
        }))
    }
    fn wrap_child(&self, ruby: &Ruby, token: Option<usize>) -> Result<Option<Value>, Error> {
        token
            .and_then(NodeId::from_token)
            .map(|id| wrap_xml_node(ruby, id, self.document))
            .transpose()
    }
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(ruby: &Ruby, rb_self: Value) -> Result<NodeId, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_self, &CLASS_XML_DOCUMENT) {
        XML_DOC_TYPE.ensure(rb_self)?; /* TypeError for any other Document */
        return Ok(xml_read(&rb_self)?.doc_node());
    }
    let nd = crate::bridge::wrapper::read_node(&XML_NODE_TYPE, rb_self)?;
    /* A wrapper's word is a real node; a null one is a broken invariant,
     * answered as `InternalError` directly rather than by a panic (which the
     * trampoline would also make one) - `XmlSelf::try_convert` reads it. */
    nd.node.xml().ok_or_else(wrapper_without_node)
}

fn wrapper_without_node() -> Error {
    Error::new(
        &crate::init::EXC_INTERNAL_ERROR,
        "an XML node wrapper without its node",
    )
}

/// `Err(TypeError)` unless `rb_doc` is an XML Document.
pub fn xml_doc_check(rb_doc: Value) -> Result<(), Error> {
    XML_DOC_TYPE.ensure(rb_doc)
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node
/// at the type boundary, like [`xml_node_unwrap`].
pub fn xml_node_document(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_self, &CLASS_XML_DOCUMENT) {
        return Ok(rb_self);
    }
    Ok(crate::bridge::wrapper::read_node(&XML_NODE_TYPE, rb_self)?.document)
}

/* ------------------------------------------------------------------ *
 * the XML receiver and the node front door                           *
 * ------------------------------------------------------------------ */

/// A method receiver already checked to be an XML node or XML Document; see the
/// HTML twin, `HtmlSelf`.
#[derive(Clone, Copy)]
pub struct XmlSelf {
    pub value: Value,
    pub id: NodeId,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl crate::bridge::rb::TryConvert for XmlSelf {
    fn try_convert(ruby: &Ruby, value: Value) -> Result<Self, Error> {
        /* One class test and one read of the wrapper: the node and its
         * Document come from the same place ([`xml_node_unwrap`] and
         * [`xml_node_document`] each ask, for the callers that need one). */
        if crate::bridge::ruby::is_kind_of(ruby, value, &CLASS_XML_DOCUMENT) {
            XML_DOC_TYPE.ensure(value)?; /* TypeError for any other Document */
            return Ok(XmlSelf {
                value,
                id: xml_read(&value)?.doc_node(),
                document: value,
            });
        }
        let nd = crate::bridge::wrapper::read_node(&XML_NODE_TYPE, value)?;
        Ok(XmlSelf {
            value,
            id: nd.node.xml().ok_or_else(wrapper_without_node)?,
            document: nd.document,
        })
    }
}

impl XmlSelf {
    /// The arena behind the receiver's Document, borrowed for reading: a
    /// counted borrow, so it must not be held across a call that can raise
    /// (`rake unsafe:boundaries`) - read what is needed, drop it, then build
    /// the Ruby result. The receiver is a method argument, which keeps its
    /// Document alive; the view borrows `self`, so it ends with the call.
    pub fn read(&self) -> Result<XmlView<'_>, Error> {
        Ok(XmlView {
            read: xml_read(&self.document)?,
            id: self.id,
            document: self.document,
        })
    }
}

/// A receiver's arena borrowed for reading, with the receiver's node:
/// [`XmlSelf::read`]. Reads as the arena.
pub struct XmlView<'a> {
    read: XmlRead<'a>,
    /// The receiver's node.
    pub id: NodeId,
    document: Value,
}

impl core::ops::Deref for XmlView<'_> {
    type Target = XmlDoc;
    #[inline]
    fn deref(&self) -> &XmlDoc {
        &self.read
    }
}

impl XmlView<'_> {
    /// The node `f` finds from the receiver's, wrapped - after the read is
    /// over, since wrapping allocates.
    pub fn wrap(
        self,
        ruby: &Ruby,
        f: impl FnOnce(&XmlDoc, NodeId) -> Option<NodeId>,
    ) -> Result<Option<Value>, Error> {
        let found = f(&self, self.id).map(|n| (n, self.type_(n)));
        let document = self.document;
        drop(self);
        found
            .map(|(n, ty)| wrap_xml_node_of_type(ruby, n, ty, document))
            .transpose()
    }

    /// The nodes `f` lists from the receiver's, as a NodeSet built with the
    /// read held ([`node_set_from_read`](crate::bridge::node_set::node_set_from_read)).
    /// `f` hands each node to the sink it is given.
    pub fn set(
        self,
        ruby: &Ruby,
        f: impl Fn(&XmlDoc, NodeId, &mut dyn FnMut(NodeId)),
    ) -> Result<Value, Error> {
        let document = self.document;
        crate::bridge::node_set::node_set_from_read(ruby, self, document, |view, out| {
            f(view, view.id, &mut |n| out(n.into()));
        })
    }
}

/// The XML arena behind `document`, for a WRITE: refused, by the guard, while
/// any read of it is held.
///
/// A write - a new node, a new byte span - can grow the arena's vectors under
/// the slices a reader borrowed. Every path that hands out a `&mut` goes
/// through here, so the one guard covers the factories and the imports as
/// well as the tree edits.
fn arena_mut(document: &Value) -> Result<XmlWrite<'_>, Error> {
    xml_write(document)
}

/// A mutation's or translation's `Result` with its failure as the Ruby
/// exception [`xml_mut_error`] maps the status to.
pub fn xml_mut_result<T>(r: Result<T, MutError>) -> Result<T, Error> {
    r.map_err(xml_mut_error)
}

/// The exception for a failed mutation's status.
fn xml_mut_error(st: MutError) -> Error {
    let msg: &str = match st {
        MutError::Oom => "out of memory mutating XML",
        MutError::BadName => {
            return crate::bridge::ruby::arg_error("not a well-formed XML name");
        }
        MutError::BadDomName(why) => return crate::bridge::ruby::arg_error(why),
        MutError::InvalidCharacter(why) => return crate::bridge::ruby::arg_error(why),
        MutError::UnboundNs => "namespace prefix is not bound in this scope",
        MutError::Type => "operation unsupported for this node type",
        MutError::PreInsert(e) => return crate::bridge::dom_error::pre_insert_error(e),
        MutError::BadNsDecl(why) => match why {
            NsDeclError::Xmlns => "namespace declaration not permitted: xmlns cannot be declared",
            NsDeclError::XmlElsewhere => {
                "namespace declaration not permitted: xml can only be bound to \
http://www.w3.org/XML/1998/namespace"
            }
            NsDeclError::ReservedUri { default: false } => {
                "namespace declaration not permitted: the XML and XMLNS namespaces cannot be \
bound to another prefix"
            }
            NsDeclError::ReservedUri { default: true } => {
                "namespace declaration not permitted: the XML and XMLNS namespaces cannot be \
the default namespace"
            }
            NsDeclError::PrefixToEmpty => {
                "namespace declaration not permitted: a prefix cannot be bound to the empty \
namespace (only xmlns=\"\" undeclares, and only the default)"
            }
        },
        MutError::DuplicateAttr => {
            "the element already has an attribute with that namespace and local name"
        }
        MutError::BadNsName => {
            "the namespace does not fit the qualified name (a prefix needs a namespace; \
xml and xmlns take only their own)"
        }
        MutError::Internal => "internal error mutating XML (no document)",
        /* The document's own budget, not the machine's memory - so the same
         * exception a parse raises for the same cause. */
        MutError::Limit => {
            return Error::new(
                &EXC_XML_LIMIT_EXCEEDED,
                "XML document exceeded its byte or node budget",
            )
        }
    };
    makiri_error(msg)
}

/* ------------------------------------------------------------------ */
/* lending the arena for an edit                                      */
/* ------------------------------------------------------------------ */

/// Run `f` on `document`'s arena to build a DETACHED node - the factories'
/// gate, and the one way a caller outside this module gets `&mut` to an XML
/// arena without a receiver.
///
/// A detached node is not in the tree, so it is not in the name index and there
/// is no receiver to check for frozenness. A TREE EDIT is [`Editing`]: it needs
/// both, and the only way to get the `&mut` for one is to spend the token
/// `begin_edit` hands out. The name says which of the two this is, because a
/// mutator reaching for "the arena, mutably" would otherwise skip the checks
/// simply by asking for the wrong thing - which is how the XML side came to have
/// nine sites that correctly skip the index invalidation and eight that must
/// not, with nothing but a reader's judgement telling them apart.
///
/// Refused while a read of the document is held (see [`arena_mut`]). The
/// `&mut` lives for `f` alone, and `f` must not run Ruby:
/// the arena is `Vec`s, so Ruby code that read or wrote this same document
/// meanwhile would alias the borrow. That is why a method converts and checks
/// its arguments FIRST and only then calls this, with nothing but engine calls
/// inside - which are Ruby-free by construction.
pub fn with_arena_for_new_node<R>(
    document: Value,
    f: impl FnOnce(&mut XmlDoc) -> R,
) -> Result<R, Error> {
    let mut write = arena_mut(&document)?;
    Ok(f(&mut write))
}

/// The receiver cleared for an edit, and the PROOF of it.
///
/// [`begin_edit`] is the only way to build one and [`Editing::with_arena`] (or
/// its attribute and data twins) the only way to spend it, so a tree edit
/// cannot reach the arena without the two things that must happen first: the
/// frozen check, and dropping the name index the edit is about to invalidate.
///
/// Spent ONCE - the three take `self` - as `HtmlEdit` is: one permit, one
/// edit, one record of it. Each spending checks again, so a second would not
/// be unsound; taking the permit narrows what a caller can get wrong.
///
/// [`with_arena_for_new_node`] stays for the FACTORIES, which build a detached node -
/// not in the tree, so not in the index, and with no receiver to freeze. The
/// HTML side has the same pair, `edit` and `HtmlEdit::node`; XML had the `&mut`
/// gate and the invalidate gate as two separate calls, and whether a site needed
/// the second was a judgement the reader had to make at each of seventeen.
pub struct Editing<'a> {
    /// The receiver, frozen-checked again at the change.
    receiver: Value,
    document: Value,
    id: NodeId,
    /// The receiver, borrowed mutably for the edit's life: a read of its
    /// document (`XmlSelf::doc_ref`) cannot still be held when the edit
    /// begins, nor be taken while it runs - the arena it borrows is the one
    /// the edit may grow. A COPY of the receiver is not covered; reads go
    /// through the receiver itself.
    _this: core::marker::PhantomData<&'a mut XmlSelf>,
}

impl Editing<'_> {
    /// The node being edited.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The arena, read while the edit is held and before it is spent: the
    /// read borrows the permit, so it is over before the change can run.
    pub fn read(&self) -> Result<XmlRead<'_>, Error> {
        xml_read(&self.document)
    }

    /// Its document, for the Ruby-side work an edit does around the arena call.
    pub fn document(&self) -> Value {
        self.document
    }

    /// Lend the arena for the change. `f` runs no Ruby (see [`with_arena_for_new_node`]),
    /// so every argument is converted BEFORE this. The frozen check is in
    /// `begin_edit`, ahead of that conversion, so a frozen receiver is reported
    /// before a bad argument, and again here. (The second check dates from
    /// when a conversion ran the argument's `#to_s`, which could freeze the
    /// receiver; no conversion runs Ruby code now.)
    ///
    /// It is also where the name index is dropped, just before `f` changes what
    /// it indexes, so nothing between `begin_edit` and here - a conversion's
    /// allocation included - can see the index gone before the tree changes.
    ///
    /// It counts as a change to a child list ([`record_edit`]); an
    /// attribute edit takes [`Editing::with_attributes`] instead, and a
    /// character-data edit [`Editing::with_data`].
    pub fn with_arena<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::ChildList, f)
    }

    /// [`Editing::with_arena`] for an insertion, which also counts as an
    /// attribute edit when placing the subtree may decide an attribute's
    /// namespace ([`crate::xml::mutate::decides_attr_ns`]): an attribute set
    /// with a prefix unbound on a detached element reads a new
    /// `namespace_uri` once its element is placed under a declaration.
    fn with_insertion<R>(
        self,
        node: NodeId,
        f: impl FnOnce(&mut XmlDoc, NodeId) -> R,
    ) -> Result<R, Error> {
        let attrs = crate::xml::mutate::decides_attr_ns(&*xml_read(&self.document)?, node);
        let also = attrs.then_some(EditKind::Attributes);
        self.lend_recording(EditKind::ChildList, also, f)
    }

    /// [`Editing::with_arena`] for an edit of ATTRIBUTES only - an element's,
    /// or an Attr node's value - which changes no child list: it counts
    /// towards the attribute version ([`record_edit`]) instead of
    /// the tree version.
    pub fn with_attributes<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::Attributes, f)
    }

    /// [`Editing::with_arena`] for an edit of a Text, Comment, CDATA or PI
    /// node's DATA, which changes no child list and no attribute: no version
    /// counts it (see [`EditKind::CharacterData`]).
    pub fn with_data<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::CharacterData, f)
    }

    fn lend<R>(self, kind: EditKind, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend_recording(kind, None, f)
    }

    /// [`Editing::lend`], recording `also` beside `kind` when the edit is
    /// both.
    fn lend_recording<R>(
        self,
        kind: EditKind,
        also: Option<EditKind>,
        f: impl FnOnce(&mut XmlDoc, NodeId) -> R,
    ) -> Result<R, Error> {
        let id = self.id;
        self.receiver.check_frozen()?;
        check_attr_owner_frozen(self.document, id)?;
        /* Recorded once both refusals are past and BEFORE the change, with no
         * Ruby run in between (`record_edit`): an edit that then fails, or a
         * panic in it, still invalidates what it may have changed. */
        record_edit(self.document, kind);
        if let Some(also) = also {
            record_edit(self.document, also);
        }
        with_arena_for_new_node(self.document, |d| {
            d.invalidate_name_index();
            f(d, id)
        })
    }
}

/// An Attr receiver's edit changes its OWNER's attribute list, so a frozen
/// owner refuses it as it refuses `delete` - checked with the receiver's own
/// frozen flag, before and after the arguments are converted.
fn check_attr_owner_frozen(document: Value, id: NodeId) -> Result<(), Error> {
    let owner = {
        let doc = xml_read(&document)?;
        match (doc.type_(id), doc.parent(id)) {
            (Some(ArenaKind::Attribute), Some(owner)) => Some(owner),
            _ => None,
        }
    };
    match owner {
        Some(owner) => crate::bridge::wrapper::check_node_frozen(document, owner),
        None => Ok(()),
    }
}

/// The receiver cleared for an edit - not frozen, its document not under
/// evaluation. The name index is dropped later, by [`Editing::with_arena`].
pub fn begin_edit<'a>(ruby: &Ruby, this: &'a mut XmlSelf) -> Result<Editing<'a>, Error> {
    this.value.check_frozen()?;
    check_attr_owner_frozen(this.document, this.id)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth(ruby, this.document);
    Ok(Editing {
        receiver: this.value,
        document: this.document,
        id: this.id,
        _this: core::marker::PhantomData,
    })
}

/// A String argument verified as an engine string - valid UTF-8, no NUL - and
/// short enough for an arena span (4 GiB).
pub fn verified_text(ruby: &Ruby, v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(ruby_verified_text(ruby, v, what)?)
}

/// `t`, unless it is too long for an XML node's `u32` span.
fn fits_xml_node<T: core::ops::Deref<Target = str>>(t: T) -> Result<T, Error> {
    if u32::try_from(t.len()).is_err() {
        return Err(makiri_error("string too long for an XML node (max 4 GiB)"));
    }
    Ok(t)
}

/// [`verified_text`] for a name given to a factory or a setter: a NUL raises
/// `ArgumentError`, as any other refused name does
/// ([`crate::bridge::string::ruby_verified_name`]).
pub fn verified_name(ruby: &Ruby, v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_name(ruby, v, what)?)
}

/// [`verified_text`] for DATA - text, comment, CDATA and PI content, an
/// attribute value: valid UTF-8, and NUL allowed, as the DOM allows it there
/// ([`crate::bridge::string::ruby_verified_data`], what HTML's data takes).
/// XML cannot write U+0000, so the serializers refuse a tree holding one, as
/// they refuse any character XML has no `Char` for.
pub fn verified_data(
    ruby: &Ruby,
    v: Value,
    what: &str,
) -> Result<crate::bridge::string::RubyData, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_data(ruby, v, what)?)
}

/// [`verified_data`] for an optional argument: `nil` is `None`.
pub fn verified_data_opt(
    ruby: &Ruby,
    v: Value,
    what: &str,
) -> Result<Option<crate::bridge::string::RubyData>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_data(ruby, v, what).map(Some)
}

/// [`verified_name`] for an optional argument: `nil` is `None`.
pub fn verified_name_opt(ruby: &Ruby, v: Value, what: &str) -> Result<Option<RubyText>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_name(ruby, v, what).map(Some)
}

/// [`verified_text`] for an optional argument: `nil` is `None` - the same
/// shape as `bridge::string::ruby_verified_text_opt`.
pub fn verified_text_opt(ruby: &Ruby, v: Value, what: &str) -> Result<Option<RubyText>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_text(ruby, v, what).map(Some)
}

/* ------------------------------------------------------------------ */
/* documents: parsing, readers, fragments                             *
 * ------------------------------------------------------------------ */

/// A `Makiri::XML::SyntaxError`-family error for a parse status.
fn parse_status_error(status: ParseError, unit: Unit) -> Error {
    match status {
        ParseError::Syntax => Error::new(&EXC_XML_SYNTAX_ERROR, unit.malformed()),
        ParseError::Limit => Error::new(&EXC_XML_LIMIT_EXCEEDED, unit.budget()),
        ParseError::Unsupported => Error::new(
            &EXC_XML_SYNTAX_ERROR,
            "unsupported DTD construct: Makiri does not apply attribute defaults or \
             non-CDATA attribute types, expand parameter entities, or expand entities \
             a DTD declares",
        ),
        ParseError::Oom => makiri_error(unit.failed()),
        /* The parser's own invariant (`ExpandErr::Overflow`: an expansion
         * outgrew its input), never the input's fault. */
        ParseError::Internal => crate::bridge::ruby::internal_error(unit.failed()),
    }
}

/// Which entry point failed. The two carry their own wording rather than one
/// composed string: the document path says "malformed XML" where the fragment
/// path says "malformed XML fragment", and the messages are observable.
#[derive(Clone, Copy)]
enum Unit {
    Document,
    Fragment,
}

impl Unit {
    fn malformed(self) -> &'static str {
        match self {
            Unit::Document => "malformed XML",
            Unit::Fragment => "malformed XML fragment",
        }
    }
    fn budget(self) -> &'static str {
        match self {
            Unit::Document => "XML document budget exceeded",
            Unit::Fragment => "XML fragment budget exceeded",
        }
    }
    fn failed(self) -> &'static str {
        match self {
            Unit::Document => "failed to parse XML document",
            Unit::Fragment => "failed to parse XML fragment",
        }
    }
}

/// Parse XML source, releasing the GVL, and wrap the result as a Document.
///
/// Runs the strict decode under the GVL first (invalid UTF-8, an undecodable
/// byte or a NUL all raise), which leaves the document in a buffer of its
/// own: the parse reads that, and no Ruby String, with the GVL released.
pub fn parse_xml_document(ruby: &Ruby, source: Value, limits: ParseLimits) -> Result<Value, Error> {
    let source = crate::bridge::ruby::string_arg(source)?;
    let src = xml_decode_input(ruby, source, Some(limits.budget()))?;
    /* The wrapper before the parse: its allocation can fail, and `src`, an
     * owned copy, is dropped on the way out if it does. */
    new_xml_document(ruby, || {
        /* Ruby-free from here: only the copied bytes and the limits cross. */
        let bytes = src.as_slice();
        let result = crate::bridge::gvl::without_gvl(ruby, || {
            crate::xml::tree::parse_ex(bytes, Some(&limits))
        })?;
        /* Gone before the Document's report to the GC, which may collect. */
        drop(src);
        result.map_err(|status| parse_status_error(status, Unit::Document))
    })
    .map(|doc| doc.value())
}

/// `Document#root` for an XML document: the root element, or nil.
pub fn document_root(ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    let root = xml_read(&rb_self)?.root();
    root.map(|n| wrap_xml_node(ruby, n, rb_self)).transpose()
}

/// `Document#internal_subset` for an XML document: the DOCTYPE node, or nil.
pub fn document_internal_subset(ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    let doctype = xml_read(&rb_self)?.doctype();
    doctype.map(|n| wrap_xml_node(ruby, n, rb_self)).transpose()
}

/// `Document#_copy`: a whole-document copy of `document`, node for node
/// (`mutate::copy_document`, where what it keeps is stated).
pub fn copy_xml_document(ruby: &Ruby, document: Value) -> Result<Value, Error> {
    /* The wrapper first, while nothing needs freeing (`new_document`). The
     * copy then reads `document` and runs no Ruby. */
    new_xml_document(ruby, || {
        crate::xml::mutate::copy_document(&*xml_read(&document)?).map_err(xml_mut_error)
    })
    .map(|doc| doc.value())
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
/// The handle holds it for a caller that fills it before handing it out
/// (`DocumentFragment.parse`).
pub fn new_empty_xml_document(ruby: &Ruby) -> Result<DocHandle, Error> {
    new_xml_document(ruby, || {
        XmlDoc::create(None).map_err(|_| makiri_error("out of memory allocating XML document"))
    })
}

/// Strict-decode `source` and parse it as a fragment into `document`'s arena,
/// returning the fragment node.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
pub fn fragment_into(
    ruby: &Ruby,
    document: Value,
    source: Value,
    inherit_doc_ns: bool,
) -> Result<NodeId, Error> {
    let max_bytes = xml_read(&document)?.max_bytes();
    let source = crate::bridge::ruby::string_arg(source)?;
    let src = xml_decode_input(ruby, source, Some(max_bytes))?;
    let parsed = {
        let mut xdoc = arena_mut(&document)?;
        tree::parse_fragment(&mut xdoc, src.as_slice(), inherit_doc_ns)
    };
    let frag = parsed.map_err(|status| parse_status_error(status, Unit::Fragment))?;
    /* The fragment grew the arena with no edit after it (a failed parse
     * rewound what it took), so it reports here, after the loan, as
     * `bridge::html`'s factories do (it may run a collection; the fragment
     * is a bare node of the arena, which `document` keeps). */
    crate::bridge::wrapper::account_growth(ruby, document);
    Ok(frag)
}

/* ------------------------------------------------------------------ */
/* attribute lookup                                                   *
 * ------------------------------------------------------------------ */

/// The attribute of `el` whose qualified name is exactly the verified `name`
/// (`AttrKey::find_in`, which counts namespace declarations as the DOM does).
///
/// `None` for a non-element (the name is then not even verified, matching the
/// readers' nil-returning behaviour). The name is converted BEFORE the arena is
/// borrowed: a Symbol's name or a transcode is a Ruby allocation.
pub fn find_attribute(ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.read()?.type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let nv = ruby_verified_text(ruby, name, "attribute name")?;
    let view = this.read()?;
    Ok(AttrKey::QName(nv.as_bytes()).find_in(&view, id))
}

/// The attribute of `el` in namespace `ns` (nil or "" for none) with local
/// name `local` - DOM "get an attribute by namespace and local name", the key
/// `remove_attribute_ns` removes by. Converted before the arena is borrowed,
/// as [`find_attribute`].
pub fn find_attribute_ns(
    ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.read()?.type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let lv = ruby_verified_text(ruby, local, "attribute local name")?;
    let nv = crate::bridge::string::namespace_arg(ruby, ns, "namespace")?;
    let key = AttrKey::Ns {
        ns: nv.as_ref().map_or(&b""[..], |n| n.as_bytes()),
        local: lv.as_bytes(),
    };
    let view = this.read()?;
    Ok(key.find_in(&view, id))
}

/* ------------------------------------------------------------------ */
/* two arenas at once: adopting and importing                         */
/* ------------------------------------------------------------------ */
/* The node methods live in `glue::xml_node::mutate`. What stays here is what
 * holds two arenas - an XML one and another, XML or HTML - at the same time,
 * which `with_arena_for_new_node`'s one-at-a-time lending cannot express. */

/// A node copied in from another document, still to be taken out of it: the
/// second half of the move `appendChild` performs across arenas. It holds the
/// source's WRITE, which [`insert`] takes before the copy is made, so
/// finishing cannot fail and cannot be skipped: there is nothing left to look
/// up or to be refused, and the write and the copy are one value rather than
/// two that must agree.
struct Adoption<'a> {
    /// The source's arena, written: the original is taken out of it, and its
    /// name index goes with it. Taken BEFORE the copy, so a refusal (a read
    /// of the source still held) comes while there is nothing to take back,
    /// rather than after the copy is in the target arena.
    sdoc: XmlWrite<'a>,
    /// The source's Document, whose tree version the removal bumps.
    src_document: Value,
    src: NodeId,
    /// The target arena's high-water mark from before the copy, so a copy
    /// the placement refuses is taken back out rather than left in the
    /// arena, holding its bytes and node budget, for every refused call.
    mark: crate::xml::arena::Mark,
    /* No field keeps the source's Document: the node is the method's
     * argument, which Ruby's stack keeps, and the node's slot keeps its
     * Document. (A `Value` field that is never read kept nothing - the
     * optimizer may drop it.) */
}

impl Adoption<'_> {
    /// The placement refused the copy: take it back out of `target`, the
    /// arena it was imported into. A refused placement wrote nothing (`place`
    /// validates and resolves before it links), so the copy is still
    /// unlinked and unwrapped, and goes as a failed copy does
    /// (`Document::rewind`); the original stays where it was.
    fn undo_copy(&self, target: &mut XmlDoc) {
        target.rewind(self.mark);
    }

    /// The placement took the copy: empty the original out of its old
    /// document, whose name index goes with it. The source's write ends
    /// with this.
    fn finish(mut self) {
        let sdoc = &mut *self.sdoc;
        /* Invalidated and recorded before the removal (`record_edit`). */
        sdoc.invalidate_name_index();
        record_edit(self.src_document, EditKind::ChildList);
        if sdoc.type_(self.src) == Some(ArenaKind::DocumentFragment) {
            while let Some(c) = sdoc.first_child(self.src) {
                remove_node(sdoc, c);
            }
        } else {
            remove_node(sdoc, self.src);
        }
    }
}

/// `node.add_child(arg)` and its siblings: put `arg` at `at` relative to the
/// receiver - moved within the document, or adopted from its own - and hand
/// back what is now in the tree: the argument, or for an adopted node its copy.
/// The HTML twin is `bridge::html::insert`.
///
/// The whole edit is the bridge's: the receiver cleared for editing, the
/// argument resolved (and copied, when it is another document's), every rule
/// checked and the namespaces resolved by the placing, and - only once that
/// has succeeded - the adopted original taken out of its own document
/// ([`Adoption::finish`]), which nothing outside this module can call.
pub fn insert(
    ruby: &Ruby,
    mut this: XmlSelf,
    arg: Value,
    at: crate::xml::mutate::Place,
) -> Result<Value, Error> {
    let edit = begin_edit(ruby, &mut this)?;
    let target_doc = edit.document();
    let (src, src_document) = incoming_ref(ruby, arg)?;
    let (node, adoption) = if crate::bridge::ruby::same_value(src_document, target_doc) {
        /* The same arena: a move. */
        (src, None)
    } else {
        /* An adoption writes the source too, so it is taken for the edit
         * BEFORE the copy is made ([`Adoption::sdoc`]). Nothing between here
         * and the release runs Ruby. */
        let sdoc = xml_write(&src_document)?;
        let (mark, copied) = {
            /* Two distinct arenas (the documents differ): the target
             * written, the source read through its own write. */
            let mut dst = arena_mut(&target_doc)?;
            (dst.mark(), import_subtree(&mut dst, &sdoc, src))
        };
        let copy = xml_mut_result(copied)?;
        (
            copy,
            Some(Adoption {
                sdoc,
                src_document,
                src,
                mark,
            }),
        )
    };
    xml_mut_result(edit.with_insertion(node, |d, target| {
        let placed = crate::xml::mutate::place(d, target, node, at);
        if let (Err(_), Some(a)) = (&placed, &adoption) {
            a.undo_copy(d);
        }
        placed
    })?)?;
    if let Some(a) = adoption {
        a.finish(); /* the source's write ends with it */
        /* The receiver's document grew by the copy. */
        crate::bridge::wrapper::account_growth(ruby, target_doc);
    }
    wrap_xml_node(ruby, node, this.document)
}

/// `arg` as the node an insertion takes in, and the Document it lives in: a
/// node of an XML document, not frozen. The copy of one from another document
/// is [`insert`]'s to make, once it holds that document's write.
fn incoming_ref(ruby: &Ruby, arg: Value) -> Result<(NodeId, Value), Error> {
    if !is_kind_of(ruby, arg, &CLASS_NODE) {
        return Err(crate::bridge::ruby::type_error(
            "expected a Makiri::XML node",
        ));
    }
    /* One read of the wrapper, as a receiver's: `TypeError` for an HTML node
     * or Document, as `xml_node_unwrap` raises. */
    let XmlSelf { id, document, .. } =
        <XmlSelf as crate::bridge::rb::TryConvert>::try_convert(ruby, arg)?;
    /* Inserting `arg` relinks IT - its parent, prev and next all change, and an
     * adoption takes it out of its own document - so a frozen argument is a
     * frozen node being modified, which the receiver check alone let through:
     * `a.remove` raised and `b.add_child(a)` did not, for the same effect on `a`.
     *
     * This reaches the nodes the caller NAMED. A fragment argument splices its
     * children, and those cannot be checked: frozenness is a property of a Ruby
     * object and the arena has no map from a node back to its wrapper (see
     * CLAUDE.md on why a per-node wrapper cache was rejected). A child the caller
     * never named is out of reach by construction, not by choice. */
    arg.check_frozen()?;
    Ok((id, document))
}

/// `Document#import_node`'s copy: `node_v` - XML from any document, or HTML -
/// copied, detached, into the XML Document `rb_self`. The source is untouched.
pub fn import_copy(
    ruby: &Ruby,
    rb_self: Value,
    node_v: Value,
    deep: bool,
) -> Result<NodeId, Error> {
    xml_doc_check(rb_self)?; /* TypeError for anything but an XML Document */
    let copy = match node_repr(node_v) {
        NodeRepr::Xml => {
            /* Read, not written: the copy goes into the receiver's arena. */
            let src_document = xml_node_document(ruby, node_v)?;
            let src = xml_node_unwrap(ruby, node_v)?;
            let copied = if crate::bridge::ruby::same_value(src_document, rb_self) {
                /* Same arena: the single-`&mut` clone path. */
                clone_node(&mut *arena_mut(&rb_self)?, src, deep)
            } else {
                let mut dst = arena_mut(&rb_self)?;
                copy_node_from(&mut dst, &*xml_read(&src_document)?, src, deep)
            };
            xml_mut_result(copied)?
        }
        NodeRepr::Html => {
            /* The HTML source, read for the closure; the copy goes into the
             * receiver's arena, another document. */
            let copied = with_arg_node(ruby, node_v, |src| {
                Ok::<_, Error>(cross_html_to_xml(&mut *arena_mut(&rb_self)?, src, deep))
            })??;
            xml_mut_result(copied)?
        }
        NodeRepr::Other => {
            return Err(crate::bridge::ruby::type_error(
                "import_node expects a Makiri node",
            ))
        }
    };
    /* A copy grows the arena with no edit after it: reported after the loan
     * (it may run a collection; the copy is a bare node of the arena). */
    crate::bridge::wrapper::account_growth(ruby, rb_self);
    Ok(copy)
}
