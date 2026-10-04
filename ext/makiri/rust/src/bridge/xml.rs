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

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{prelude::*, Error, Value};

use crate::bridge::html::with_arg_node;
use crate::bridge::ruby::{check_frozen, is_kind_of, value};
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::*;
use crate::bridge::wrapper::{
    ensure_document_mutable, node_repr, DocKind, DocumentShell, NodeRepr,
};
use crate::bridge::xml_decode::xml_decode_input_value;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT, EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::init::{
    CLASS_XML_ATTR, CLASS_XML_CDATA_SECTION, CLASS_XML_COMMENT, CLASS_XML_DOCUMENT_FRAGMENT,
    CLASS_XML_DOCUMENT_TYPE, CLASS_XML_ELEMENT, CLASS_XML_NODE, CLASS_XML_PROCESSING_INSTRUCTION,
    CLASS_XML_TEXT,
};
use crate::lexbor::adapter::cross_import::cross_html_to_xml;
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
pub fn wrap_xml_node(id: NodeId, document: Value) -> Result<Value, Error> {
    let ty = arena_ref(&document).type_(id);
    if ty == Some(ArenaKind::Document) {
        return Ok(document);
    }
    let klass = XML_NODE_CLASSES.class_for(ty.map_or(CrateKind::Other, Into::into));

    crate::bridge::wrapper::wrap_cached(&XML_NODE_TYPE, klass, id, document)
}

impl crate::bridge::wrapper::NodeHandleSource for NodeId {
    fn identity(&self) -> usize {
        self.to_token()
    }

    fn into_handle(self, _document: Value) -> NodeHandle {
        NodeHandle::Xml(self)
    }
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(rb_self: Value) -> Result<NodeId, Error> {
    if crate::bridge::ruby::is_kind_of(rb_self, &CLASS_XML_DOCUMENT) {
        XML_DOC_TYPE.get(&rb_self)?; /* TypeError for any other Document */
        // SAFETY: the arena a live XML Document owns.
        return Ok(unsafe { (*doc_of(rb_self)).doc_node() });
    }
    let nd: &NodeData = XML_NODE_TYPE.get(&rb_self)?;
    /* A wrapper's word is a real node; a null one is a broken invariant. It
     * is answered as `InternalError` directly, not by panicking: this runs in
     * magnus's argument conversion (`XmlSelf::try_convert`), outside
     * `entry`, where a panic would be an unrescuable `fatal`. */
    nd.node.xml().ok_or_else(|| {
        Error::new(
            crate::init::EXC_INTERNAL_ERROR.exception(),
            "an XML node wrapper without its node",
        )
    })
}

/// The XML arena behind a value checked to be an XML Document:
/// `Err(TypeError)` for anything else. For a receiver not yet established as
/// one; [`doc_of`] is for a document already known to be.
pub fn xml_doc_unwrap(rb_doc: Value) -> Result<*mut XmlDoc, Error> {
    XML_DOC_TYPE.get(&rb_doc)?;
    Ok(doc_of(rb_doc))
}

/// The XML arena behind an XML Document.
///
/// Every XML Document HAS one: `DocumentShell::install` gives it the arena
/// before the Document reaches Ruby, and nothing takes it away. So there is no
/// "no arena" case to handle, and a null here is a broken invariant - it
/// panics (unwinding to `fatal` / `Makiri::InternalError`) rather than being
/// read through.
pub fn doc_of(document: Value) -> *mut XmlDoc {
    let arena = xml_arena_known(document);
    assert!(!arena.is_null(), "an XML Document without its arena");
    arena
}

/// The arena behind `document`, borrowed for as long as the caller borrows the
/// VALUE - which it holds, and which keeps the Document alive.
fn arena_ref(document: &Value) -> &XmlDoc {
    // SAFETY: the Document's `DocData` owns the arena, alive while `document`
    // is held; only read here.
    unsafe { &*doc_of(*document) }
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node
/// at the type boundary, like [`xml_node_unwrap`].
pub fn xml_node_document(rb_self: Value) -> Result<Value, Error> {
    if crate::bridge::ruby::is_kind_of(rb_self, &CLASS_XML_DOCUMENT) {
        return Ok(rb_self);
    }
    let nd: &NodeData = XML_NODE_TYPE.get(&rb_self)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
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

impl magnus::TryConvert for XmlSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let id = xml_node_unwrap(value)?;
        let document = xml_node_document(value)?;
        Ok(XmlSelf {
            value,
            id,
            document,
        })
    }
}

impl XmlSelf {
    /// The arena behind the receiver's Document, borrowed (readers) for as long
    /// as the receiver is.
    pub fn doc_ref(&self) -> &XmlDoc {
        arena_ref(&self.document)
    }
}

/// The XML arena behind `document`, for a WRITE: refused while an XPath
/// evaluation with a handler is reading the document.
///
/// The evaluator holds the arena as `&Document` for the whole walk, and a
/// write - a new node, a new byte span - can grow the arena's vectors under the
/// slices it borrowed. Every path that hands out a `&mut` goes through here, so
/// the one mutation gate covers the factories and the imports as well as the
/// tree edits.
fn arena_mut(document: Value) -> Result<*mut XmlDoc, Error> {
    ensure_document_mutable(document)?;
    Ok(doc_of(document))
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
                EXC_XML_LIMIT_EXCEEDED.exception(),
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
/// Refused while an XPath evaluation with a handler reads the document (see
/// [`arena_mut`]). The `&mut` lives for `f` alone, and `f` must not run Ruby:
/// the arena is `Vec`s, so Ruby code that read or wrote this same document
/// meanwhile would alias the borrow. That is why a method converts and checks
/// its arguments FIRST and only then calls this, with nothing but engine calls
/// inside - which are Ruby-free by construction.
pub fn with_arena_for_new_node<R>(
    document: Value,
    f: impl FnOnce(&mut XmlDoc) -> R,
) -> Result<R, Error> {
    let xd = arena_mut(document)?;
    // SAFETY: a live arena of `document`, which the caller holds; cleared for
    // writing above, and borrowed only for `f`, which runs no Ruby.
    Ok(f(unsafe { &mut *xd }))
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
pub struct Editing {
    /// The receiver, frozen-checked again at the change.
    receiver: Value,
    document: Value,
    id: NodeId,
}

impl Editing {
    /// The node being edited.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Its document, for the Ruby-side work an edit does around the arena call.
    pub fn document(&self) -> Value {
        self.document
    }

    /// Lend the arena for the change. `f` runs no Ruby (see [`with_arena_for_new_node`]),
    /// so every argument is converted BEFORE this. The frozen check is in
    /// `begin_edit`, ahead of that conversion, so a frozen receiver is reported
    /// before a bad argument - and again here, because the conversion's `#to_s`
    /// may have frozen the receiver since.
    ///
    /// It is also where the name index is dropped, just before `f` changes what
    /// it indexes. Not in `begin_edit`: the argument conversion between the two
    /// runs `#to_s`, and a query there rebuilt the index from the tree about to
    /// change - `//a` then kept finding an element renamed to `b`.
    ///
    /// It counts as a change to a child list ([`record_edit`]); an
    /// attribute edit takes [`Editing::with_attributes`] instead, and a
    /// character-data edit [`Editing::with_data`].
    pub fn with_arena<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::ChildList, f)
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
        let id = self.id;
        check_frozen(self.receiver)?;
        check_attr_owner_frozen(self.document, id)?;
        ensure_document_mutable(self.document)?;
        /* Recorded once both refusals are past and BEFORE the change, with no
         * Ruby run in between (`record_edit`): an edit that then fails, or a
         * panic in it, still invalidates what it may have changed. */
        record_edit(self.document, kind);
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
    let doc = arena_ref(&document);
    match (doc.type_(id), doc.parent(id)) {
        (Some(ArenaKind::Attribute), Some(owner)) => {
            crate::bridge::wrapper::check_node_frozen(document, owner)
        }
        _ => Ok(()),
    }
}

/// The receiver cleared for an edit - not frozen, its document not under
/// evaluation. The name index is dropped later, by [`Editing::with_arena`].
pub fn begin_edit(this: XmlSelf) -> Result<Editing, Error> {
    check_frozen(this.value)?;
    check_attr_owner_frozen(this.document, this.id)?;
    /* The evaluation guard, checked now so it is reported before a bad
     * argument; `with_arena` checks it again at the change. */
    ensure_document_mutable(this.document)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth(this.document);
    Ok(Editing {
        receiver: this.value,
        document: this.document,
        id: this.id,
    })
}

/// A String argument verified as an engine string - valid UTF-8, no NUL - and
/// short enough for an arena span (4 GiB).
pub fn verified_text(v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(ruby_verified_text(v, what)?)
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
pub fn verified_name(v: Value, what: &str) -> Result<RubyText, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_name(v, what)?)
}

/// [`verified_text`] for DATA - text, comment, CDATA and PI content, an
/// attribute value: valid UTF-8, and NUL allowed, as the DOM allows it there
/// ([`crate::bridge::string::ruby_verified_data`], what HTML's data takes).
/// XML cannot write U+0000, so the serializers refuse a tree holding one, as
/// they refuse any character XML has no `Char` for.
pub fn verified_data(v: Value, what: &str) -> Result<crate::bridge::string::RubyData, Error> {
    fits_xml_node(crate::bridge::string::ruby_verified_data(v, what)?)
}

/// [`verified_data`] for an optional argument: `nil` is `None`.
pub fn verified_data_opt(
    v: Value,
    what: &str,
) -> Result<Option<crate::bridge::string::RubyData>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_data(v, what).map(Some)
}

/// [`verified_name`] for an optional argument: `nil` is `None`.
pub fn verified_name_opt(v: Value, what: &str) -> Result<Option<RubyText>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_name(v, what).map(Some)
}

/// [`verified_text`] for an optional argument: `nil` is `None` - the same
/// shape as `bridge::string::ruby_verified_text_opt`.
pub fn verified_text_opt(v: Value, what: &str) -> Result<Option<RubyText>, Error> {
    if v.is_nil() {
        return Ok(None);
    }
    verified_text(v, what).map(Some)
}

/* ------------------------------------------------------------------ */
/* documents: parsing, readers, fragments                             *
 * ------------------------------------------------------------------ */

/// A `Makiri::XML::SyntaxError`-family error for a parse status.
fn parse_status_error(status: ParseError, unit: Unit) -> Error {
    match status {
        ParseError::Syntax => Error::new(EXC_XML_SYNTAX_ERROR.exception(), unit.malformed()),
        ParseError::Limit => Error::new(EXC_XML_LIMIT_EXCEEDED.exception(), unit.budget()),
        ParseError::Unsupported => Error::new(
            EXC_XML_SYNTAX_ERROR.exception(),
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
/// byte or a NUL all raise), then copies into a private buffer BEFORE the
/// wrapper exists, so no GC point can run between obtaining the decoded String
/// and copying it.
pub fn parse_xml_document(source: Value, limits: ParseLimits) -> Result<Value, Error> {
    let source = crate::bridge::ruby::string_of(source)?;
    let decoded = xml_decode_input_value(source, Some(limits.budget()))?;
    let src = crate::bridge::string::ruby_string_bytes(decoded)?;

    /* The wrapper first, while nothing needs freeing (see DocumentShell). The
     * source is already copied, so this Ruby allocation cannot disturb it. */
    let shell = DocumentShell::new(DocKind::Xml);

    /* Ruby-free from here: only the copied bytes and the limits cross. */
    let result = crate::bridge::gvl::without_gvl(|| tree::parse_ex(src.as_slice(), Some(&limits)))?;
    drop(src);

    let arena = result.map_err(|status| parse_status_error(status, Unit::Document))?;
    /* `src` is gone, so the collection `install`'s GC report may trigger
     * disturbs nothing. */
    Ok(shell.install_xml(arena))
}

/// `Document#root` for an XML document: the root element, or nil.
pub fn document_root(rb_self: Value) -> Result<Option<Value>, Error> {
    arena_ref(&rb_self)
        .root()
        .map(|n| wrap_xml_node(n, rb_self))
        .transpose()
}

/// `Document#internal_subset` for an XML document: the DOCTYPE node, or nil.
pub fn document_internal_subset(rb_self: Value) -> Result<Option<Value>, Error> {
    arena_ref(&rb_self)
        .doctype()
        .map(|n| wrap_xml_node(n, rb_self))
        .transpose()
}

/// `Document#_copy`: a whole-document copy of `document`, node for node
/// (`mutate::copy_document`, where what it keeps is stated).
pub fn copy_xml_document(document: Value) -> Result<Value, Error> {
    /* The wrapper first, while nothing needs freeing - see DocumentShell. The
     * copy then reads `document` and runs no Ruby. */
    let shell = DocumentShell::new(DocKind::Xml);
    let arena = crate::xml::mutate::copy_document(arena_ref(&document)).map_err(xml_mut_error)?;
    Ok(shell.install_xml(arena))
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
pub fn new_empty_xml_document() -> Result<Value, Error> {
    let shell = DocumentShell::new(DocKind::Xml);
    let arena =
        XmlDoc::create(None).map_err(|_| makiri_error("out of memory allocating XML document"))?;
    Ok(shell.install_xml(arena))
}

/// Strict-decode `source` and parse it as a fragment into `document`'s arena,
/// returning the fragment node.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
pub fn fragment_into(
    document: Value,
    source: Value,
    inherit_doc_ns: bool,
) -> Result<NodeId, Error> {
    /* Refused first, so a document an evaluation is reading answers that
     * rather than whatever the argument is... */
    let xdoc = arena_mut(document)?;
    let source = crate::bridge::ruby::string_of(source)?;
    // SAFETY: a live arena; the decode only reads its `max_bytes`.
    let decoded = xml_decode_input_value(source, Some(unsafe { (*xdoc).max_bytes() }))?;
    let src = crate::bridge::string::ruby_string_bytes(decoded)?;
    /* ...and checked again: the conversion is arbitrary Ruby, which can start
     * an evaluation that suspends mid-walk (an Enumerator) still holding
     * slices of the arena this parse grows. */
    let xdoc = arena_mut(document)?;
    // SAFETY: the arena is live and mutable for this call, under the GVL.
    tree::parse_fragment(unsafe { &mut *xdoc }, src.as_slice(), inherit_doc_ns)
        .map_err(|status| parse_status_error(status, Unit::Fragment))
}

/* ------------------------------------------------------------------ */
/* attribute lookup                                                   *
 * ------------------------------------------------------------------ */

/// The attribute of `el` whose qualified name is exactly the verified `name`
/// (`AttrKey::find_in`, which counts namespace declarations as the DOM does).
///
/// `None` for a non-element (the name is then not even verified, matching the
/// readers' nil-returning behaviour). The name is converted BEFORE the arena is
/// borrowed, because its `to_str` is Ruby code and it may edit this same
/// document.
pub fn find_attribute(this: XmlSelf, name: Value) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.doc_ref().type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let nv = ruby_verified_text(name, "attribute name")?;
    Ok(AttrKey::QName(nv.as_bytes()).find_in(this.doc_ref(), id))
}

/// The attribute of `el` in namespace `ns` (nil or "" for none) with local
/// name `local` - DOM "get an attribute by namespace and local name", the key
/// `remove_attribute_ns` removes by. Converted before the arena is borrowed,
/// as [`find_attribute`].
pub fn find_attribute_ns(this: XmlSelf, ns: Value, local: Value) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.doc_ref().type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let lv = ruby_verified_text(local, "attribute local name")?;
    let nv = crate::bridge::string::namespace_arg(ns, "namespace")?;
    let key = AttrKey::Ns {
        ns: nv.as_ref().map_or(&b""[..], |n| n.as_bytes()),
        local: lv.as_bytes(),
    };
    Ok(key.find_in(this.doc_ref(), id))
}

/* ------------------------------------------------------------------ */
/* two arenas at once: adopting and importing                         */
/* ------------------------------------------------------------------ */
/* The node methods live in `glue::xml_node::mutate`. What stays here is what
 * holds two arenas - or an arena and a Lexbor document - at the same time,
 * which `with_arena_for_new_node`'s one-at-a-time lending cannot express. */

/// A node copied in from another document, still to be taken out of it: the
/// second half of the move `appendChild` performs across arenas. It carries
/// the source arena and node [`incoming_node`] already resolved, so finishing
/// cannot fail - there is nothing left to look up.
struct Adoption {
    src_doc: *mut XmlDoc,
    /// The source's Document, whose tree version the removal bumps.
    src_document: Value,
    src: NodeId,
    /// The source node's wrapper, which keeps its document - and so
    /// `src_doc` - alive until the adoption is finished.
    _keep: Value,
}

impl Adoption {
    /// Empty the node out of its old document, whose name index goes with it.
    fn finish(self) {
        // SAFETY: `src_doc` is the live arena `incoming_node` found and cleared
        // for writing; `_keep` holds it, and the caller ran only engine code
        // on the OTHER arena since.
        let sdoc = unsafe { &mut *self.src_doc };
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
pub fn insert(this: XmlSelf, arg: Value, at: crate::xml::mutate::Place) -> Result<Value, Error> {
    let edit = begin_edit(this)?;
    let (node, adoption) = incoming_node(edit.document(), arg)?;
    xml_mut_result(edit.with_arena(|d, target| crate::xml::mutate::place(d, target, node, at))?)?;
    if let Some(a) = adoption {
        a.finish();
    }
    wrap_xml_node(node, this.document)
}

/// `arg` as a node of `target_doc`'s arena: itself when it already lives there
/// (a move), or a copy imported from its own document plus the [`Adoption`]
/// that takes it out of there once it is placed.
fn incoming_node(target_doc: Value, arg: Value) -> Result<(NodeId, Option<Adoption>), Error> {
    if !is_kind_of(arg, &CLASS_NODE) || !is_kind_of(xml_node_document(arg)?, &CLASS_XML_DOCUMENT) {
        return Err(crate::bridge::ruby::type_error(
            "expected a Makiri::XML node (NodeSet / String arguments are a later phase)",
        ));
    }
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
    check_frozen(arg)?;
    let src = xml_node_unwrap(arg)?;
    let src_document = xml_node_document(arg)?;
    if src_document.as_raw() == target_doc.as_raw() {
        return Ok((src, None)); /* same arena -> move */
    }
    let xd = arena_mut(target_doc)?;
    /* The source changes too - adopting takes the node out of it. */
    let src_doc = arena_mut(src_document)?;
    // SAFETY: two distinct live arenas (the documents differ), both cleared
    // for writing; the source is only read here.
    let copy = xml_mut_result(unsafe { import_subtree(&mut *xd, &*src_doc, src) })?;
    Ok((
        copy,
        Some(Adoption {
            src_doc,
            src_document,
            src,
            _keep: arg,
        }),
    ))
}

/// `Document#import_node`'s copy: `node_v` - XML from any document, or HTML -
/// copied, detached, into the XML Document `rb_self`. The source is untouched.
pub fn import_copy(rb_self: Value, node_v: Value, deep: bool) -> Result<NodeId, Error> {
    xml_doc_unwrap(rb_self)?; /* TypeError for anything but an XML Document */
    let xd = arena_mut(rb_self)?;
    let copy = match node_repr(node_v) {
        NodeRepr::Xml => {
            /* Read, not written: the copy goes into the receiver's arena. */
            let src_doc = doc_of(xml_node_document(node_v)?);
            let src = xml_node_unwrap(node_v)?;
            if src_doc == xd {
                /* Same arena: the single-`&mut` clone path. */
                // SAFETY: the target arena, which is the source here.
                xml_mut_result(unsafe { clone_node(&mut *xd, src, deep) })?
            } else {
                // SAFETY: two distinct live arenas.
                xml_mut_result(unsafe { copy_node_from(&mut *xd, &*src_doc, src, deep) })?
            }
        }
        NodeRepr::Html => {
            /* The HTML source, live for the closure; the copy goes into the
             * receiver's arena, another document. */
            let copied = with_arg_node(node_v, |src| {
                // SAFETY: the receiver's live arena - another document than
                // the source's, so the source's borrow does not overlap it.
                cross_html_to_xml(unsafe { &mut *xd }, src, deep)
            })?;
            xml_mut_result(copied)?
        }
        NodeRepr::Other => {
            return Err(crate::bridge::ruby::type_error(
                "import_node expects a Makiri node",
            ))
        }
    };
    Ok(copy)
}
