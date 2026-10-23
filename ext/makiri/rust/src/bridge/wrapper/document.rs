//! The Document wrapper: its data, its TypedData types and GC hooks, the
//! node cache, [`DocumentShell`], and the accessors that reach a Document's
//! content.

#![allow(unsafe_code)]

use core::cell::{Cell, OnceCell, RefCell};

use crate::bridge::ruby::{internal_error, makiri_error};
use magnus::{Error, Value};

use super::*;
use crate::bridge::dom::{Body, Conflict, HtmlRead, HtmlWrite, XmlRead, XmlWrite};
use crate::bridge::ruby::{value, VALUE};
use crate::bridge::typed::{Held, Hooks, Tracer, TypedType};
use crate::child_index::ChildPositionMemo;
use crate::falloc::{MapInsert, Reserve};
use crate::html_dom::post_parse::HtmlParsed;
use crate::init::RbConst;
use crate::node_type::NodeType;
use crate::token::Kind;
use crate::xml::model::Document as XmlDoc;
use core::hash::BuildHasherDefault;
use std::collections::HashMap;

/* ------------------------------------------------------------------ *
 * the document wrapper                                               *
 * ------------------------------------------------------------------ */

/// One Ruby wrapper per node, so navigating to the same node twice gives the
/// SAME object.
///
/// Without it every navigation allocated a fresh wrapper, and everything that
/// lives on a Ruby object was silently lost: `equal?` was false for one node,
/// an instance variable set through one wrapper was gone through the next, a
/// singleton method vanished, and `freeze` protected only the object you
/// happened to be holding. `==`/`eql?`/`hash` were unaffected, because those
/// are node identity, which is why it went unnoticed.
///
/// The cost is Nokogiri's, and it is the same cost for the same reason: a
/// wrapper stays alive while its document does. Measured on a 50,000-node
/// document, minor GC after wrapping N nodes and dropping every reference:
/// N=1,000 costs nothing (0.30 ms, the same as N=0), N=50,000 costs +2.2 ms.
/// Nokogiri's figures for the same experiment are 0.31 ms and +2.24 ms.
///
/// Keyed by the node TOKEN - a `NodeId` for XML, a node pointer for HTML - which
/// is stable for the document's life in both: XML never recycles an arena slot,
/// and HTML detaches without destroying, so a node is never freed.
pub(super) struct NodeCache {
    /// Empty until the first navigation, so a document nobody walks pays
    /// nothing.
    /// Tokens are a slot index or an aligned pointer - already well
    /// distributed - so they are mixed ([`crate::ptr_table::MixHasher`]) rather
    /// than hashed.
    map: HashMap<usize, Held, BuildHasherDefault<crate::ptr_table::MixHasher>>,
}

impl NodeCache {
    fn get(&self, token: usize) -> Option<Value> {
        // SAFETY: an entry of this cache, which the Document's trace hands
        // over movable (marked, then relocated by the same walk); the caller
        // holds the Document.
        self.map.get(&token).map(|v| unsafe { v.get() })
    }

    /// Room for one more wrapper, so the [`NodeCache::insert`] after it
    /// cannot fail - see [`DocData::reserve_node`].
    fn reserve(&mut self) -> Result<(), ()> {
        self.map.falloc_reserve(1)
    }

    /// Remember `wrapper` as the one wrapper for `token`. After
    /// [`NodeCache::reserve`] this allocates nothing, so it does not fail.
    fn insert(&mut self, token: usize, wrapper: Held) -> Result<(), ()> {
        self.map.falloc_insert(token, wrapper)
    }

    /// MOVABLE, not pinned: a document walked end to end holds one entry per
    /// node, and pinning them all would stop compaction doing its job.
    fn trace(&self, tracer: &Tracer) {
        for v in self.map.values() {
            tracer.movable(v);
        }
    }

    fn memsize(&self) -> usize {
        self.map.capacity() * (core::mem::size_of::<usize>() + core::mem::size_of::<Held>())
    }
}

/// A Document's edit bookkeeping: the versions readers key their caches by.
/// Only [`record_edit`] writes them.
#[derive(Clone, Copy, Default)]
pub(super) struct EditState {
    /// `Document#tree_version`: the child-list edits ([`EditKind::ChildList`]).
    pub(super) tree_version: u64,
    /// `Document#attribute_version`: the attribute edits
    /// ([`EditKind::Attributes`]).
    pub(super) attribute_version: u64,
}

impl EditState {
    /// Count `kind`'s edit by the version that counts it - by at most one, so
    /// a reader keying a cache by one version is not refilled for another's
    /// edits.
    pub(super) fn record(&mut self, kind: EditKind) {
        match kind {
            EditKind::ChildList => self.tree_version = self.tree_version.wrapping_add(1),
            EditKind::Attributes => self.attribute_version = self.attribute_version.wrapping_add(1),
            EditKind::CharacterData => {}
        }
    }
}

/// What the GC was last told about a Document's content, which lives outside
/// Ruby's allocator ([`account_document`]): `Drop` takes back exactly `bytes`,
/// and [`account_growth`] compares against it.
#[derive(Clone, Copy, Default)]
pub(super) struct ExternalReport {
    /// The external bytes reported.
    bytes: usize,
    /// An HTML document's pool chunk count when `bytes` was measured: what
    /// [`account_growth`] compares against, since the byte count itself costs
    /// a walk of every chunk.
    chunks: usize,
}

impl ExternalReport {
    /// Record a measurement of `now` bytes in `chunks` chunks, and answer the
    /// difference from the last one for the GC - `None` when there is none to
    /// report, or a size Ruby could not address (a truncated difference would
    /// unbalance the release).
    pub(super) fn update(&mut self, now: usize, chunks: usize) -> Option<isize> {
        let (Ok(now_i), Ok(then_i)) = (isize::try_from(now), isize::try_from(self.bytes)) else {
            return None;
        };
        self.chunks = chunks;
        let diff = now_i.wrapping_sub(then_i);
        if diff == 0 {
            return None;
        }
        self.bytes = now;
        Some(diff)
    }

    /// What to report when the content is freed: everything reported, back.
    fn release(&self) -> Option<isize> {
        isize::try_from(self.bytes).ok().map(isize::wrapping_neg)
    }
}

/// A Document wrapper's data: the parsed content (owned - GC frees it), the
/// mutation gate's count, and the reserved errors Array.
///
/// Read and written through `&DocData` only - every field that changes is a
/// cell - so a method may hold one while another reads it (a wrapper made
/// while a child walk holds the memo, a GC run in the middle of either), and
/// the GC callbacks read it shared too. The two `RefCell`s are borrowed only
/// for spans that run no Ruby and so no collection; the mark and compaction
/// hooks rely on that (see [`Hooks for DocData`](#impl-Hooks-for-DocData)).
pub struct DocData {
    /// Set once, by `DocumentShell::install`; reached only through the
    /// guards of `bridge::dom` ([`html_read`], [`html_write`], [`xml_read`],
    /// [`xml_write`]).
    content: OnceCell<Body>,
    /// The mutation gate and the edit versions ([`EditState`]).
    pub(super) edits: Cell<EditState>,
    errors: Held,
    /// What the GC has been told about the content ([`ExternalReport`]).
    pub(super) report: Cell<ExternalReport>,
    /// One wrapper per node; see [`NodeCache`].
    ///
    /// Boxed and optional so a document nobody navigates never allocates a
    /// cache, and its wrapper stays one pointer wide.
    nodes: RefCell<Option<Box<NodeCache>>>,
    /// Where `child_count` / `child_at` last were ([`ChildPositionMemo`]).
    /// Allocates nothing until the first of them records.
    pub(super) child_memo: RefCell<ChildPositionMemo>,
}

impl DocData {
    /// The one wrapper for `token`, or None until something navigates to it.
    fn cached(&self, token: usize) -> Result<Option<Value>, Error> {
        let nodes = self.nodes.try_borrow().map_err(|_| busy_cache())?;
        Ok(nodes.as_ref().and_then(|c| c.get(token)))
    }

    /// Make room in the cache (allocating it on first use) for the wrapper
    /// about to be built. This is the step that can fail, and it comes
    /// BEFORE the wrapper exists: a wrapper made and then not cached would be
    /// a second object for its node on the next navigation, without the
    /// first one's `freeze`, instance variables or singleton methods.
    fn reserve_node(&self) -> Result<(), ()> {
        let mut nodes = self.nodes.try_borrow_mut().map_err(|_| ())?;
        if nodes.is_none() {
            *nodes = Some(
                crate::falloc::try_box(NodeCache {
                    map: HashMap::with_hasher(BuildHasherDefault::default()),
                })
                .map_err(|_| ())?,
            );
        }
        nodes.as_mut().ok_or(())?.reserve()
    }

    /// Remember `wrapper` for `token`, in the room [`DocData::reserve_node`]
    /// made.
    fn cache(&self, token: usize, wrapper: Held) -> Result<(), ()> {
        let mut nodes = self.nodes.try_borrow_mut().map_err(|_| ())?;
        nodes.as_mut().ok_or(())?.insert(token, wrapper)
    }

    /// Change the edit state in place - the one read-modify-write of the
    /// cell, which holds a `Copy` value.
    pub(super) fn update_edits(&self, f: impl FnOnce(&mut EditState)) {
        let mut edits = self.edits.get();
        f(&mut edits);
        self.edits.set(edits);
    }

    /// The Document's parse-warning Array.
    pub fn errors(&self) -> Value {
        // SAFETY: a field of this Document's data, which its mark marks; the
        // caller reached it through the Document, which it holds.
        unsafe { self.errors.get() }
    }

    /// Whether the content has grown enough since the last report to measure
    /// and report it again ([`account_growth`]): by an eighth, and at least
    /// [`GROWTH_MIN_CHUNKS`] chunks (HTML) or [`GROWTH_MIN_BYTES`] (XML).
    /// An edit holding the content answers no: its growth is measured when
    /// the next edit begins.
    pub(super) fn grown_enough(&self) -> bool {
        match self.content.get() {
            None => false,
            Some(body @ Body::Html(_)) => {
                let then = self.report.get().chunks;
                body.arena_chunks().is_some_and(|now| {
                    now >= then.saturating_add((then / 8).max(GROWTH_MIN_CHUNKS))
                })
            }
            Some(body @ Body::Xml(_)) => {
                let then = self.report.get().bytes;
                body.external_bytes()
                    .is_some_and(|now| now >= then.saturating_add((then / 8).max(GROWTH_MIN_BYTES)))
            }
        }
    }

    /// The HTML content's chunk count, 0 for XML; `None` while an edit holds
    /// it.
    pub(super) fn arena_chunks(&self) -> Option<usize> {
        self.content.get().map_or(Some(0), Body::arena_chunks)
    }

    /// The bytes the content holds outside Ruby's allocator, 0 with none;
    /// `None` while an edit holds it.
    pub(super) fn external_bytes(&self) -> Option<usize> {
        self.content.get().map_or(Some(0), Body::external_bytes)
    }
}

/// The node cache's `RefCell` is borrowed mutably only by `reserve_node` and
/// `cache`, whose spans allocate from the system allocator alone - no Ruby,
/// so no collection - and a collection therefore never finds it borrowed. Were
/// it to, marking the cache partly would let the GC free a wrapper it still
/// hands out, so the hooks abort instead, as a broken invariant in a GC
/// callback must (see the note on the callbacks in `typed.rs`).
impl Hooks for DocData {
    fn trace(&self, tracer: &Tracer) {
        tracer.pinned(&self.errors);
        match self.nodes.try_borrow() {
            Ok(nodes) => {
                if let Some(cache) = nodes.as_ref() {
                    cache.trace(tracer);
                }
            }
            Err(_) => std::process::abort(),
        }
    }

    fn memsize(&self) -> usize {
        /* A size only, so a cell found borrowed counts as empty. */
        let cache = self
            .nodes
            .try_borrow()
            .map_or(0, |n| n.as_ref().map_or(0, |c| c.memsize()));
        let memo = self.child_memo.try_borrow().map_or(0, |m| m.memsize());
        /* The content - or, were an edit holding it, which a collection
         * cannot find (an edit runs no Ruby) but a size need not insist on,
         * what was last reported for it. */
        let content = self
            .external_bytes()
            .unwrap_or_else(|| self.report.get().bytes);
        core::mem::size_of::<DocData>()
            .saturating_add(content)
            .saturating_add(cache)
            .saturating_add(memo)
    }
}

/// Run by the free callback, in Ruby's collector - so it must not panic (the
/// callback aborts rather than unwind into C) and must touch no Ruby object.
/// A `DocData` exists only in the memory `TypedType::wrap` gave it, so this
/// is the one place a Document's content is freed - by drop glue, after this
/// body, with the node cache.
impl Drop for DocData {
    fn drop(&mut self) {
        /* Balance the report, or the GC keeps counting freed arenas as live
         * and collects ever more eagerly. A plain C call, as this hook has to
         * be: it only subtracts, and Ruby's own `xfree` does the same from
         * here. */
        if let Some(diff) = self.report.get().release() {
            crate::bridge::ruby::report_external_bytes(diff);
        }
    }
}

/// The base type: the kind-agnostic accessors (`doc_content`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: TypedType<DocData> = TypedType::base(c"Makiri::Document".as_ptr());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as an HTML document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
pub static HTML_DOC_TYPE: TypedType<DocData> =
    TypedType::derived(c"Makiri::HTML::Document".as_ptr(), &DOC_TYPE);
pub static XML_DOC_TYPE: TypedType<DocData> =
    TypedType::derived(c"Makiri::XML::Document".as_ptr(), &DOC_TYPE);

/// Which leaf class a Document wrapper is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    Html,
    Xml,
}

impl DocKind {
    /// The kind of `document`, a live Makiri Document of either leaf.
    ///
    /// Read from the wrapped TypedData type rather than the Ruby class, so a
    /// subclass of either Document answers the kind its bytes are.
    pub fn of(document: Value) -> DocKind {
        if XML_DOC_TYPE.is(document) {
            DocKind::Xml
        } else {
            DocKind::Html
        }
    }
}

impl From<DocKind> for Kind {
    #[inline]
    fn from(kind: DocKind) -> Kind {
        match kind {
            DocKind::Html => Kind::Arena,
            DocKind::Xml => Kind::Xml,
        }
    }
}

/// A Document wrapper allocated before the content it will own, and the only
/// way a Document gets that content.
///
/// The order is the point. Allocating the Ruby wrapper can fail
/// (`NoMemoryError`, as `Err`), and a parse result is expensive to have made
/// for nothing. So the wrapper is made FIRST - before the parse - and the
/// parsed handle goes in afterwards through
/// [`install`](Self::install), which also reports the arena to the GC. That
/// report is not optional (see `account_document`), and the parse entries used
/// to make it by hand, each one a place to forget it.
///
/// Until `install` the wrapper is HIDDEN (`rb_obj_hide`: no class, so
/// `ObjectSpace` does not list it): a Document with no content answers
/// nothing, and every accessor assumes the content is there. A parse runs
/// with the GVL released between the two, so another thread could otherwise
/// find the wrapper and call a method on it; a parse that fails leaves it
/// hidden for the GC to free.
pub struct DocumentShell {
    obj: VALUE,
    klass: VALUE,
}

impl DocumentShell {
    pub fn new(kind: DocKind) -> Result<DocumentShell, Error> {
        let (klass, ty) = match kind {
            DocKind::Html => (crate::init::CLASS_HTML_DOCUMENT.raw(), &HTML_DOC_TYPE),
            DocKind::Xml => (crate::init::CLASS_XML_DOCUMENT.raw(), &XML_DOC_TYPE),
        };
        /* The errors array is built before the wrap and kept in a local, so the
         * conservative stack scan pins it across the wrap's allocation and the
         * store closure does not allocate (an allocation there could raise
         * NoMemoryError while the caller holds a live resource). */
        let errors = magnus::value::ReprValue::as_value(crate::bridge::build::ary(0)?);
        // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
        let obj = unsafe {
            ty.wrap(
                klass,
                || DocData {
                    content: OnceCell::new(),
                    edits: Cell::new(EditState::default()),
                    errors: Held::empty(),
                    report: Cell::new(ExternalReport::default()),
                    nodes: RefCell::new(None),
                    child_memo: RefCell::new(ChildPositionMemo::default()),
                },
                |d, store| d.errors = store.hold(errors),
            )
        }?;
        // SAFETY: a live object of ours, not yet handed out; hiding clears its
        // class field and raises nothing.
        unsafe { rb_sys::rb_obj_hide(obj) };
        Ok(DocumentShell { obj, klass })
    }

    /// Give the HTML Document its parsed document - the GC owns it from here -
    /// and report the arena's size.
    pub fn install_html(self, parsed: Box<HtmlParsed>) -> Value {
        self.install(Body::Html(crate::doc_cell::DocCell::new(parsed)))
    }

    /// Give the XML Document its arena, as [`install_html`](Self::install_html).
    pub fn install_xml(self, arena: Box<XmlDoc>) -> Value {
        self.install(Body::Xml(crate::doc_cell::DocCell::new(arena)))
    }

    #[allow(
        clippy::panic,
        reason = "`new` makes the wrapper without content, and only `install` - which takes the shell - gives it some"
    )]
    fn install(self, content: Body) -> Value {
        // SAFETY: `self.obj` is a live Document wrapper (the base type matches
        // either leaf) that has no content yet, so nothing is overwritten.
        let rb_doc = unsafe { value(self.obj) };
        if DOC_TYPE.get_known(&rb_doc).content.set(content).is_err() {
            panic!("a Document installed twice");
        }
        account_document(self.obj);
        /* Content in place: the Document can be seen now. */
        // SAFETY: the object `new` hid, given back the class it was made
        // with; revealing raises nothing.
        unsafe { rb_sys::rb_obj_reveal(self.obj, self.klass) };
        // SAFETY: a live Document.
        unsafe { value(self.obj) }
    }
}

/* ------------------------------------------------------------------ *
 * document content <-> wrapper accessors                             *
 * ------------------------------------------------------------------ */

/// The content of `rb_doc`, a VALUE known to be a Document (a node's
/// keepalive Document, or a Document method's receiver). Every Document that
/// Ruby can reach has one (`DocumentShell` reveals it only after `install`),
/// so its absence is a broken invariant.
fn body_known(rb_doc: &Value) -> Result<&Body, Error> {
    DOC_TYPE
        .get_known(rb_doc)
        .content
        .get()
        .ok_or_else(|| internal_error("a Document without its content"))
}

/// Why a guard could not be taken, as the exception it raises.
fn held_error(held: Conflict) -> Error {
    match held {
        Conflict::ByEdit => internal_error("a Document was read while an edit held it"),
        /* A reader a Ruby call left suspended (a Fiber). */
        Conflict::ByRead => makiri_error(
            "cannot modify a document while a read of it is in progress (suspended in Ruby)",
        ),
    }
}

/// A Document of the wrong kind for the guard asked for: the caller vouched
/// for the kind, so a broken invariant.
fn wrong_kind() -> Error {
    internal_error("a Document of the other kind")
}

/// `rb_doc`'s HTML document, borrowed for reading for as long as the caller
/// borrows the VALUE - which it holds, and which keeps the Document alive.
/// `rb_doc` is known to be an HTML Document.
pub fn html_read(rb_doc: &Value) -> Result<HtmlRead<'_>, Error> {
    match body_known(rb_doc)? {
        Body::Html(c) => HtmlRead::of(c).map_err(held_error),
        Body::Xml(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s HTML document, borrowed for an edit; refused while a read
/// holds it. The caller has made the edit's checks and runs no Ruby while it holds this.
pub(in crate::bridge) fn html_write(rb_doc: &Value) -> Result<HtmlWrite<'_>, Error> {
    match body_known(rb_doc)? {
        Body::Html(c) => HtmlWrite::of(c).map_err(held_error),
        Body::Xml(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s XML arena, borrowed for reading, as [`html_read`].
pub fn xml_read(rb_doc: &Value) -> Result<XmlRead<'_>, Error> {
    match body_known(rb_doc)? {
        Body::Xml(c) => XmlRead::of(c).map_err(held_error),
        Body::Html(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s XML arena, borrowed for an edit, as [`html_write`].
pub(in crate::bridge) fn xml_write(rb_doc: &Value) -> Result<XmlWrite<'_>, Error> {
    match body_known(rb_doc)? {
        Body::Xml(c) => XmlWrite::of(c).map_err(held_error),
        Body::Html(_) => Err(wrong_kind()),
    }
}

/// Whether `rb_doc`'s content is borrowed right now - by a read or an edit.
/// Between Ruby calls it never is, unless a raise skipped a guard's release:
/// what `Makiri.__borrowed?` answers the raise sweep.
pub fn document_borrowed(rb_doc: Value) -> Result<bool, Error> {
    Ok(match DOC_TYPE.get(&rb_doc)?.content.get() {
        Some(Body::Html(c)) => c.is_borrowed(),
        Some(Body::Xml(c)) => c.is_borrowed(),
        None => false,
    })
}

/// `Err(TypeError)` unless `rb_doc` is an HTML Document.
pub fn html_doc_check(rb_doc: Value) -> Result<(), Error> {
    HTML_DOC_TYPE.get(&rb_doc).map(|_| ())
}

/// Run `f` over the parsed document of a VALUE known to be an HTML Document,
/// for a read confined to `f` - which must run no Ruby. An index a reader
/// builds is a cell (`HtmlParsed`), so readers take the document shared and
/// may overlap.
pub(in crate::bridge) fn with_html_parsed_known<R>(
    rb_doc: Value,
    f: impl FnOnce(&HtmlParsed) -> R,
) -> Result<R, Error> {
    let read = html_read(&rb_doc)?;
    Ok(f(read.parsed()))
}

/// [`with_html_parsed_known`] for a Document of either kind: `None` for an
/// XML one.
pub(in crate::bridge) fn with_html_parsed_if<R>(
    rb_doc: Value,
    f: impl FnOnce(&HtmlParsed) -> R,
) -> Result<Option<R>, Error> {
    match body_known(&rb_doc)? {
        Body::Html(c) => {
            let read = HtmlRead::of(c).map_err(held_error)?;
            Ok(Some(f(read.parsed())))
        }
        Body::Xml(_) => Ok(None),
    }
}

/// One representation's leaf classes, by node type: the mapping both
/// `wrap_*_node` functions make, written once. The two tables differ only in
/// which classes they name, so neither can grow a kind the other lacks.
pub(in crate::bridge) struct NodeClasses {
    pub node: &'static RbConst<magnus::RClass>,
    pub element: &'static RbConst<magnus::RClass>,
    pub attr: &'static RbConst<magnus::RClass>,
    pub text: &'static RbConst<magnus::RClass>,
    pub comment: &'static RbConst<magnus::RClass>,
    pub cdata: &'static RbConst<magnus::RClass>,
    pub pi: &'static RbConst<magnus::RClass>,
    pub doctype: &'static RbConst<magnus::RClass>,
    pub fragment: &'static RbConst<magnus::RClass>,
}

impl NodeClasses {
    /// The class a node of type `t` is wrapped as; the representation's
    /// abstract `Node` for a type with no leaf of its own (entity, notation,
    /// an unknown number). A DOCUMENT is the caller's to map back onto the
    /// Ruby Document before asking.
    #[inline]
    pub fn class_for(&self, t: NodeType) -> VALUE {
        match t {
            NodeType::Element => self.element,
            NodeType::Attribute => self.attr,
            NodeType::Text => self.text,
            NodeType::Comment => self.comment,
            NodeType::CDataSection => self.cdata,
            NodeType::Pi => self.pi,
            NodeType::DocumentType => self.doctype,
            NodeType::DocumentFragment => self.fragment,
            _ => self.node,
        }
        .raw()
    }
}

/// The one wrapper of class `klass` (a `ty` object) for a node under
/// `document`: the cached one, or a fresh one built from `source` and then
/// cached. The shared half of the two `wrap_*_node` functions.
///
/// The candidate supplies both its cache identity and, on a miss, its stored
/// handle. HTML key minting is therefore skipped on a cache hit, while the
/// token and handle cannot come from different nodes.
///
/// One wrapper per node: navigating to a node twice must give the SAME object,
/// or everything that lives on a Ruby object is silently lost - `equal?`, an
/// instance variable, a singleton method, `freeze`. A Document is already its
/// own wrapper, which is why it needs no entry. So a wrapper is handed out
/// only once it is cached: `Err` (out of memory) when the cache cannot make
/// room, never an uncached object.
///
/// The caller vouches that the node is a node of `document` of the
/// representation `ty` wraps, and `klass` a class of it: only the two
/// `wrap_*_node` functions call this, each for its own kind.
pub(in crate::bridge) fn wrap_cached(
    ty: &'static TypedType<NodeData>,
    klass: VALUE,
    source: impl NodeHandleSource,
    document: Value,
) -> Result<Value, Error> {
    let token = source.identity();
    if let Some(cached) = cached_node(document, token)? {
        return Ok(cached);
    }
    /* Room for the entry first: a failure after the wrap would hand out a
     * wrapper the cache does not know (DocData::reserve_node). */
    doc_data_known(&document)
        .reserve_node()
        .map_err(|_| wrap_oom())?;
    let node = source.into_handle(document);
    /* The Document is stored after the wrap: see `TypedType::wrap`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    let fresh = unsafe {
        value(ty.wrap(
            klass,
            || NodeData {
                node,
                document: Held::empty(),
            },
            |nd, store| nd.document = store.hold(document),
        )?)
    };
    /* After the wrap, so the VALUE exists, into the room reserved above - no
     * allocation. Were it to fail anyway, `fresh` is dropped unseen rather
     * than handed out uncached. */
    cache_node(document, token, fresh).map_err(|_| wrap_oom())?;
    Ok(fresh)
}

fn wrap_oom() -> Error {
    makiri_error("out of memory wrapping a node")
}

/// The one wrapper for `token` under `rb_doc`, or None the first time.
///
/// `rb_doc` must be a live Document; [`wrap_cached`] is the only caller and
/// already holds one.
fn cached_node(rb_doc: Value, token: usize) -> Result<Option<Value>, Error> {
    doc_data_known(&rb_doc).cached(token)
}

/// `FrozenError` when the node `source` names has a wrapper and that wrapper is
/// frozen. A node with no wrapper yet cannot have been frozen. For an edit
/// that reaches a node other than its receiver - an Attr's owner element,
/// whose attribute list an Attr's `remove` or `content=` changes.
pub(in crate::bridge) fn check_node_frozen(
    rb_doc: Value,
    source: impl NodeHandleSource,
) -> Result<(), Error> {
    match cached_node(rb_doc, source.identity())? {
        Some(wrapper) => crate::bridge::ruby::check_frozen(wrapper),
        None => Ok(()),
    }
}

/// Remember `wrapper` as the one wrapper for `token` under `rb_doc`.
fn cache_node(rb_doc: Value, token: usize, wrapper: Value) -> Result<(), ()> {
    DOC_TYPE.with_store(rb_doc, |d, store| d.cache(token, store.hold(wrapper)))
}

/// A Document's wrapper data, for the fields that are the wrapper's own
/// rather than the content's - every one of them a cell, so this is shared.
pub(super) fn doc_data_known(rb_doc: &Value) -> &DocData {
    DOC_TYPE.get_known(rb_doc)
}

/// The node cache was found borrowed: a broken invariant (no borrow of it
/// spans Ruby), not a bad input.
fn busy_cache() -> Error {
    crate::bridge::ruby::internal_error("a Document's node cache was found in use")
}
