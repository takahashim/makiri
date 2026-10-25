//! The Document wrapper: its data, its TypedData types and GC hooks, the
//! node table, [`DocumentShell`], and the accessors that reach a Document's
//! content.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use core::cell::{Cell, OnceCell, RefCell};

use crate::bridge::rb::{Error, Value};
use crate::bridge::ruby::{internal_error, makiri_error};

use super::*;
use crate::bridge::dom::{Body, Conflict, HtmlRead, HtmlWrite, XmlRead, XmlWrite};
use crate::bridge::typed::{Payload, TypedType};
use crate::child_index::ChildPositionMemo;
use crate::html_dom::post_parse::HtmlParsed;
use crate::init::RbConst;
use crate::node_type::NodeType;
use crate::token::Kind;
use crate::xml::model::Document as XmlDoc;

/* ------------------------------------------------------------------ *
 * the document wrapper                                               *
 * ------------------------------------------------------------------ */

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
/// Ruby's allocator ([`account_document`]): the free callback takes back exactly
/// `bytes` (`Payload::reported_external`),
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
}

/// A Document wrapper's data: the parsed content (owned - GC frees it), the
/// mutation gate's count, and the node table. The parse-warning Array is a
/// hidden ivar of the object (`Document#errors` is read rarely).
///
/// Read and written through `&DocData` only - every field that changes is a
/// cell - so a method may hold one while another reads it (a wrapper made
/// while a child walk holds the memo, a GC run in the middle of either), and
/// the GC callbacks read it shared too.
pub struct DocData {
    /// Set once, by `DocumentShell::install`; reached only through the
    /// guards of `bridge::dom` ([`html_read`], [`html_write`], [`xml_read`],
    /// [`xml_write`]).
    content: OnceCell<Body>,
    /* The node table is not here: it is the object's slot 0 ([`DocType`],
     * read by `node_table`). */
    /// The mutation gate and the edit versions ([`EditState`]).
    pub(super) edits: Cell<EditState>,
    /// The node table: one Ruby wrapper per node, so navigating to the same
    /// node twice gives the SAME object. Without it every navigation made a
    /// fresh wrapper, and everything that lives on a Ruby object was lost:
    /// `equal?`, an instance variable, a singleton method, `freeze`.
    ///
    /// What the GC has been told about the content ([`ExternalReport`]).
    pub(super) report: Cell<ExternalReport>,
    /// Where `child_count` / `child_at` last were ([`ChildPositionMemo`]).
    /// Allocates nothing until the first of them records.
    pub(super) child_memo: RefCell<ChildPositionMemo>,
}

impl DocData {
    /// Change the edit state in place - the one read-modify-write of the
    /// cell, which holds a `Copy` value.
    pub(super) fn update_edits(&self, f: impl FnOnce(&mut EditState)) {
        let mut edits = self.edits.get();
        f(&mut edits);
        self.edits.set(edits);
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

impl Payload for DocData {
    fn memsize(&self) -> usize {
        /* A size only, so a cell found borrowed counts as empty. */
        let memo = self.child_memo.try_borrow().map_or(0, |m| m.memsize());
        /* The content - or, were an edit holding it, which a collection
         * cannot find (an edit runs no Ruby) but a size need not insist on,
         * what was last reported for it. */
        let content = self
            .external_bytes()
            .unwrap_or_else(|| self.report.get().bytes);
        core::mem::size_of::<DocData>()
            .saturating_add(content)
            .saturating_add(memo)
    }

    /// Everything reported for the content, which the free callback gives
    /// back once the content is dropped - or the GC keeps counting freed
    /// arenas as live and collects ever more eagerly.
    fn reported_external(&self) -> usize {
        self.report.get().bytes
    }
}

/// A Document wrapper's type: [`DocData`], and in slot 0 the node table.
///
/// The node table: one Ruby wrapper per node, so navigating to the same node
/// twice gives the SAME object. Without it every navigation made a fresh
/// wrapper, and everything that lives on a Ruby object was lost: `equal?`,
/// an instance variable, a singleton method, `freeze`. A Ruby Hash from the
/// node's TOKEN (a `NodeId` for XML, an arena node word for HTML, stable for
/// the document's life: neither arena reuses a node) to its wrapper - a Ruby
/// Hash rather than a Rust map, so the GC marks the wrappers and moves them
/// itself, and the one slot is all the Document marks. The cost is
/// Nokogiri's: a wrapper lives as long as its document.
pub type DocType = TypedType<DocData, 1>;

/// The base type: the kind-agnostic accessors (`doc_content`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: DocType = TypedType::base(c"Makiri::Document".as_ptr());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as an HTML document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
pub static HTML_DOC_TYPE: DocType =
    TypedType::derived(c"Makiri::HTML::Document".as_ptr(), &DOC_TYPE);
pub static XML_DOC_TYPE: DocType = TypedType::derived(c"Makiri::XML::Document".as_ptr(), &DOC_TYPE);

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
    obj: Value,
    klass: crate::bridge::rb::RClass,
}

impl DocumentShell {
    pub fn new(ruby: &Ruby, kind: DocKind) -> Result<DocumentShell, Error> {
        let (klass, ty) = match kind {
            DocKind::Html => (
                crate::init::CLASS_HTML_DOCUMENT.defined(ruby)?,
                &HTML_DOC_TYPE,
            ),
            DocKind::Xml => (
                crate::init::CLASS_XML_DOCUMENT.defined(ruby)?,
                &XML_DOC_TYPE,
            ),
        };
        /* The errors array and the node table are built before the wrap and
         * kept in locals, so the conservative stack scan pins them across the
         * wrap's allocation; the table goes into the slot with the data. */
        let errors = crate::bridge::rb::ReprValue::as_value(crate::bridge::build::ary(ruby, 0)?);
        let nodes = crate::bridge::rb::ReprValue::as_value(crate::bridge::raw::hash_new(ruby)?);
        /* Hidden, so no Ruby code reaches the table through `ObjectSpace` to
         * clear, freeze or fill it: its values are handed out as node
         * wrappers unchecked. */
        crate::bridge::ruby::hide(nodes);
        let obj = ty.wrap(
            ruby,
            klass,
            DocData {
                content: OnceCell::new(),
                edits: Cell::new(EditState::default()),
                report: Cell::new(ExternalReport::default()),
                child_memo: RefCell::new(ChildPositionMemo::default()),
            },
            [nodes],
        )?;
        /* Hidden whether or not the ivar write succeeds: a failure there must
         * not leave a content-less Document where `ObjectSpace` lists it. */
        let errors_set = crate::bridge::raw::ivar_set(obj, ivar_errors(), errors);
        crate::bridge::ruby::hide(obj);
        errors_set?;
        Ok(DocumentShell { obj, klass })
    }

    /// Give the HTML Document its parsed document - the GC owns it from here -
    /// and report the arena's size.
    pub fn install_html(self, ruby: &crate::bridge::rb::Ruby, parsed: Box<HtmlParsed>) -> Value {
        self.install(ruby, Body::Html(crate::doc_cell::DocCell::new(parsed)))
    }

    /// Give the XML Document its arena, as [`install_html`](Self::install_html).
    pub fn install_xml(self, ruby: &crate::bridge::rb::Ruby, arena: Box<XmlDoc>) -> Value {
        self.install(ruby, Body::Xml(crate::doc_cell::DocCell::new(arena)))
    }

    #[allow(
        clippy::panic,
        reason = "`new` makes the wrapper without content, and only `install` - which takes the shell - gives it some"
    )]
    fn install(self, ruby: &crate::bridge::rb::Ruby, content: Body) -> Value {
        /* `self.obj` is a Document wrapper with no content yet, so nothing is
         * overwritten. */
        if DOC_TYPE.get_known(&self.obj).content.set(content).is_err() {
            panic!("a Document installed twice");
        }
        account_document(ruby, self.obj);
        /* Content in place: the Document can be seen now. */
        crate::bridge::ruby::reveal(self.obj, self.klass);
        self.obj
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
    pub node: &'static RbConst<crate::bridge::rb::RClass>,
    pub element: &'static RbConst<crate::bridge::rb::RClass>,
    pub attr: &'static RbConst<crate::bridge::rb::RClass>,
    pub text: &'static RbConst<crate::bridge::rb::RClass>,
    pub comment: &'static RbConst<crate::bridge::rb::RClass>,
    pub cdata: &'static RbConst<crate::bridge::rb::RClass>,
    pub pi: &'static RbConst<crate::bridge::rb::RClass>,
    pub doctype: &'static RbConst<crate::bridge::rb::RClass>,
    pub fragment: &'static RbConst<crate::bridge::rb::RClass>,
}

impl NodeClasses {
    /// The class a node of type `t` is wrapped as; the representation's
    /// abstract `Node` for a type with no leaf of its own (entity, notation,
    /// an unknown number). A DOCUMENT is the caller's to map back onto the
    /// Ruby Document before asking.
    #[inline]
    pub fn class_for(&self, ruby: &Ruby, t: NodeType) -> Result<crate::bridge::rb::RClass, Error> {
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
        .defined(ruby)
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
/// only once it is in the table: `Err` (`NoMemoryError`) when the entry
/// cannot be made, and the fresh wrapper, which nothing refers to, is left
/// for the GC - never an object outside the table.
///
/// The caller vouches that the node is a node of `document` of the
/// representation `ty` wraps, and `klass` a class of it: only the two
/// `wrap_*_node` functions call this, each for its own kind.
pub(in crate::bridge) fn wrap_cached(
    ruby: &Ruby,
    ty: &'static NodeWrapperType,
    klass: crate::bridge::rb::RClass,
    source: impl NodeHandleSource,
    document: Value,
) -> Result<Value, Error> {
    let token = source.identity();
    let table = node_table(document)?;
    if let Some(cached) = crate::bridge::raw::table_get(table, token)? {
        return Ok(cached);
    }
    let node = source.into_handle(document);
    let fresh = ty.wrap(ruby, klass, NodeData { node }, [document])?;
    /* The table entry: a Ruby reference, which the GC marks and moves
     * itself. A failure leaves `fresh` unseen and unreferenced. */
    crate::bridge::raw::table_set(table, token, fresh)?;
    Ok(fresh)
}

/// The Document's node table (see `DocumentShell::new`).
fn node_table(document: Value) -> Result<crate::bridge::rb::RHash, Error> {
    crate::bridge::rb::RHash::from_value(DOC_TYPE.root_known::<0>(&document))
        .ok_or_else(|| internal_error("a Document without its node table"))
}

/// The one wrapper for `token` under `rb_doc`, or None the first time.
fn cached_node(rb_doc: Value, token: usize) -> Result<Option<Value>, Error> {
    crate::bridge::raw::table_get(node_table(rb_doc)?, token)
}

static IVAR_DOCUMENT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static IVAR_ERRORS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static IVAR_NODE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The hidden ivar an XPathContext keeps its Document in.
pub(in crate::bridge) fn ivar_document() -> crate::bridge::ruby::ID {
    IVAR_DOCUMENT.load(core::sync::atomic::Ordering::Relaxed) as crate::bridge::ruby::ID
}

/// The hidden ivar a Document keeps its parse-warning Array in.
pub(in crate::bridge) fn ivar_errors() -> crate::bridge::ruby::ID {
    IVAR_ERRORS.load(core::sync::atomic::Ordering::Relaxed) as crate::bridge::ruby::ID
}

/// The hidden ivar an XPathContext keeps its context node in.
pub(in crate::bridge) fn ivar_node() -> crate::bridge::ruby::ID {
    IVAR_NODE.load(core::sync::atomic::Ordering::Relaxed) as crate::bridge::ruby::ID
}

/// Intern the hidden ivars' names. From `Init_makiri`.
pub fn init_ivars(ruby: &Ruby) -> Result<(), Error> {
    let d = crate::bridge::raw::intern_cstr(ruby, c"makiri_document")?;
    let e = crate::bridge::raw::intern_cstr(ruby, c"makiri_errors")?;
    let n = crate::bridge::raw::intern_cstr(ruby, c"makiri_node")?;
    IVAR_DOCUMENT.store(d as usize, core::sync::atomic::Ordering::Relaxed);
    IVAR_ERRORS.store(e as usize, core::sync::atomic::Ordering::Relaxed);
    IVAR_NODE.store(n as usize, core::sync::atomic::Ordering::Relaxed);
    Ok(())
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

/// A Document's wrapper data, for the fields that are the wrapper's own
/// rather than the content's - every one of them a cell, so this is shared.
pub(super) fn doc_data_known(rb_doc: &Value) -> &DocData {
    DOC_TYPE.get_known(rb_doc)
}
