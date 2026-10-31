//! The Document wrapper: its data, its TypedData types and GC hooks,
//! [`new_html_document`] / [`new_xml_document`], and the accessors that reach
//! a Document's content. The node table in its slot is `node_table`'s.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use core::cell::{Cell, OnceCell, RefCell};

use crate::bridge::rb::{Error, Value};
use crate::bridge::ruby::{internal_error, makiri_error};

use super::*;
use crate::bridge::dom::{Body, Conflict, HtmlRead, HtmlWrite, XmlRead, XmlWrite};
use crate::bridge::typed::{Birth, Payload, TypedType};
use crate::child_index::ChildPositionMemo;
use crate::html_dom::post_parse::HtmlParsed;
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

/// A Document wrapper's data: the parsed content (owned - GC frees it), the
/// edit versions, what was reported to the GC, and the child-position memo.
/// The node table is the object's slot 0 ([`DocType`]), and the
/// parse-warning Array a hidden ivar (`Document#errors` is read rarely).
///
/// Read and written through `&DocData` only - every field that changes is a
/// cell - so a method may hold one while another reads it (a wrapper made
/// while a child walk holds the memo, a GC run in the middle of either), and
/// the GC callbacks read it shared too.
pub struct DocData {
    /// Set once, by `new_document`; reached only through the
    /// guards of `bridge::dom` ([`html_read`], [`html_write`], [`xml_read`],
    /// [`xml_write`]).
    content: OnceCell<Body>,
    /* The node table is not here: it is the object's slot 0 ([`DocType`],
     * read by `node_table`). */
    /// The edit versions ([`EditState`]).
    pub(super) edits: Cell<EditState>,
    /// What the GC has been told about the content, which lives outside
    /// Ruby's allocator ([`account_document`]): the free callback takes back
    /// exactly that, and [`account_growth`] compares against it.
    pub(super) external: crate::bridge::typed::External,
    /// An HTML document's pool chunk count when the content was last
    /// measured: what [`account_growth`] compares against, since the byte
    /// count itself costs a walk of every chunk.
    pub(super) chunks: Cell<usize>,
    /// Where `child_count` / `child_at` last were ([`ChildPositionMemo`]).
    /// Allocates nothing until the first of them records.
    pub(super) child_memo: RefCell<ChildPositionMemo>,
    /// The content's document stamp, set with it (0 before): the node table
    /// is keyed by a node's index alone, and a lookup is for a node of this
    /// document only when the stamp in its handle is this one.
    stamp: Cell<u32>,
}

impl DocData {
    /// The content's document stamp, 0 before it is set: whose nodes the
    /// node table holds ([`super::node_table`]).
    pub(super) fn stamp(&self) -> u32 {
        self.stamp.get()
    }

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
                let then = self.chunks.get();
                body.arena_chunks().is_some_and(|now| {
                    now >= then.saturating_add((then / 8).max(GROWTH_MIN_CHUNKS))
                })
            }
            Some(body @ Body::Xml(_)) => {
                let then = self.external.reported();
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
            .unwrap_or_else(|| self.external.reported());
        core::mem::size_of::<DocData>()
            .saturating_add(content)
            .saturating_add(memo)
    }

    /// Everything reported for the content, which the free callback gives
    /// back once the content is dropped - or the GC keeps counting freed
    /// arenas as live and collects ever more eagerly.
    fn reported_external(&self) -> usize {
        self.external.reported()
    }
}

/// A Document wrapper's type: [`DocData`], and in slot 0 the node table.
///
/// The node table: one Ruby wrapper per node, so navigating to the same node
/// twice gives the SAME object. Without it every navigation made a fresh
/// wrapper, and everything that lives on a Ruby object was lost: `equal?`,
/// an instance variable, a singleton method, `freeze`. A Ruby Hash from the
/// node's index in its document (stable for the document's life: neither
/// arena reuses a node; the document's stamp is checked against the node's
/// apart from the key, `node_table::table_and_key`) to its wrapper - a Ruby
/// Hash rather than a Rust map, so the GC marks the wrappers and moves them
/// itself, and the one slot is all the Document marks. The cost is
/// Nokogiri's: a wrapper lives as long as its document.
pub type DocType = TypedType<DocData, 1>;

/// A Document held while it is used ([`crate::bridge::typed::Typed`]):
/// what [`new_html_document`] / [`new_xml_document`] return, so a caller that
/// works on the new Document before handing it to Ruby keeps it meanwhile.
pub type DocHandle = crate::bridge::typed::Typed<DocData, 1>;

/// The base type: the kind-agnostic accessors (`doc_content`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: DocType = TypedType::base(c"Makiri::Document");

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so an HTML-only check
/// ([`html_doc_check`], `HTML_DOC_TYPE.check`) raises TypeError on an XML
/// Document through Ruby's own type machinery.
pub static HTML_DOC_TYPE: DocType = TypedType::derived(c"Makiri::HTML::Document", &DOC_TYPE);
pub static XML_DOC_TYPE: DocType = TypedType::derived(c"Makiri::XML::Document", &DOC_TYPE);

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

/// A new HTML Document, holding what `make` builds - the only way a
/// Document gets its content (see [`new_document`]).
pub fn new_html_document(
    ruby: &Ruby,
    make: impl FnOnce() -> Result<Box<HtmlParsed>, Error>,
) -> Result<DocHandle, Error> {
    new_document(ruby, DocKind::Html, || {
        let parsed = make()?;
        let stamp = parsed.doc().arena().stamp();
        Ok((stamp, Body::Html(crate::doc_cell::DocCell::new(parsed))))
    })
}

/// A new XML Document, holding the arena `make` builds, as
/// [`new_html_document`].
pub fn new_xml_document(
    ruby: &Ruby,
    make: impl FnOnce() -> Result<Box<XmlDoc>, Error>,
) -> Result<DocHandle, Error> {
    new_document(ruby, DocKind::Xml, || {
        let arena = make()?;
        let stamp = arena.doc_node().stamp();
        Ok((stamp, Body::Xml(crate::doc_cell::DocCell::new(arena))))
    })
}

/// A Document wrapper made before the content it will own, `make` run to
/// build the content, and the content installed and reported to the GC.
///
/// The order is the point. Allocating the Ruby wrapper can fail
/// (`NoMemoryError`, as `Err`), and a parse result is expensive to have made
/// for nothing - and is leaked if a raise passes it. So the wrapper is made
/// FIRST, and `make` - the parse - runs after, inside `wrap_hidden`'s fill.
/// The arena's report to the GC (`account_document`) is not optional, and
/// the parse entries used to make it by hand, each one a place to forget it.
///
/// Until the content is in, the wrapper is HIDDEN (`TypedType::wrap_hidden`:
/// no class, so `ObjectSpace` does not list it, and it is handed out only
/// once shown): a Document with no content answers nothing, and every
/// accessor assumes the content is there. A parse runs with the GVL released
/// inside `make`, so another thread could otherwise find the wrapper and
/// call a method on it; a `make` that fails leaves it hidden for the GC to
/// free.
fn new_document(
    ruby: &Ruby,
    kind: DocKind,
    make: impl FnOnce() -> Result<(u32, Body), Error>,
) -> Result<DocHandle, Error> {
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
    /* Hidden, so no Ruby code reaches the table through `ObjectSpace` to
     * clear, freeze or fill it: its values are handed out as node
     * wrappers unchecked. */
    let nodes = crate::nupur::table::IntTable::new_hidden(ruby)?;
    ty.wrap_hidden(
        ruby,
        Birth {
            klass,
            data: DocData {
                content: OnceCell::new(),
                edits: Cell::new(EditState::default()),
                external: crate::bridge::typed::External::new(),
                chunks: Cell::new(0),
                child_memo: RefCell::new(ChildPositionMemo::default()),
                stamp: Cell::new(0),
            },
            roots: [nodes],
        },
        /* Written before it is hidden, and a failure hides it all the
         * same: no content-less Document is left where `ObjectSpace`
         * lists it. */
        &[(ivar_errors(), errors)],
        |d| {
            let (stamp, content) = make()?;
            /* A wrapper just made, with no content yet. */
            if d.content.set(content).is_err() {
                return Err(internal_error("a Document installed twice"));
            }
            d.stamp.set(stamp);
            /* Reported through the handle `wrap_hidden` holds the object
             * by: nothing else refers to it yet, and the report may collect. */
            account_data(ruby, d);
            Ok(())
        },
    )
}

/* ------------------------------------------------------------------ *
 * document content <-> wrapper accessors                             *
 * ------------------------------------------------------------------ */

/// `rb_doc`, a VALUE known to be a Document (a node's keepalive Document,
/// or a Document method's receiver), as the handle its content is borrowed
/// through: the guards below borrow it, so the Document is held for as long
/// as they are. A mismatch is a broken invariant, and panics.
#[inline]
pub fn doc_known(rb_doc: Value) -> DocHandle {
    DOC_TYPE.known(rb_doc)
}

/// The content of `doc`. Every Document that Ruby can reach has one
/// (`new_document` shows it only once the content is in), so its absence is
/// a broken invariant.
fn body_known(doc: &DocHandle) -> Result<&Body, Error> {
    doc.data()
        .content
        .get()
        .ok_or_else(|| internal_error("a Document without its content"))
}

/// Why a guard could not be taken, as the exception it raises.
fn held_error(held: Conflict) -> Error {
    match held {
        Conflict::ByEdit => internal_error("a Document was read while an edit held it"),
        /* A read still held when an edit begins - a copy of the receiver's,
         * say; no Ruby code runs under a read, so none is left suspended. */
        Conflict::ByRead => {
            makiri_error("cannot modify a document while a read of it is in progress")
        }
    }
}

/// A Document of the wrong kind for the guard asked for: the caller vouched
/// for the kind, so a broken invariant.
fn wrong_kind() -> Error {
    internal_error("a Document of the other kind")
}

/// `doc`'s HTML document, borrowed for reading for as long as the handle -
/// which holds the Document - is. `doc` is known to be an HTML Document.
pub fn html_read(doc: &DocHandle) -> Result<HtmlRead<'_>, Error> {
    match body_known(doc)? {
        Body::Html(c) => HtmlRead::of(c).map_err(held_error),
        Body::Xml(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s HTML document, borrowed for an edit; refused while a read
/// holds it. The caller has made the edit's checks and runs no Ruby while it holds this.
pub(in crate::bridge) fn html_write(doc: &DocHandle) -> Result<HtmlWrite<'_>, Error> {
    match body_known(doc)? {
        Body::Html(c) => HtmlWrite::of(c).map_err(held_error),
        Body::Xml(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s XML arena, borrowed for reading, as [`html_read`].
pub fn xml_read(doc: &DocHandle) -> Result<XmlRead<'_>, Error> {
    match body_known(doc)? {
        Body::Xml(c) => XmlRead::of(c).map_err(held_error),
        Body::Html(_) => Err(wrong_kind()),
    }
}

/// `rb_doc`'s XML arena, borrowed for an edit, as [`html_write`].
///
/// A write - a new node, a new byte span - can grow the arena's vectors under
/// the slices a reader borrowed, so it is refused while any read is held.
/// Every `&mut` to an XML arena is one of these - a tree edit's, a factory's
/// and an import's alike - so the one guard covers them all.
pub(in crate::bridge) fn xml_write(doc: &DocHandle) -> Result<XmlWrite<'_>, Error> {
    match body_known(doc)? {
        Body::Xml(c) => XmlWrite::of(c).map_err(held_error),
        Body::Html(_) => Err(wrong_kind()),
    }
}

/// Whether `rb_doc`'s content is borrowed right now - by a read or an edit.
/// Between Ruby calls it never is, unless a raise skipped a guard's release:
/// what `Makiri.__borrowed?` answers the raise sweep.
pub fn document_borrowed(rb_doc: Value) -> Result<bool, Error> {
    Ok(match DOC_TYPE.check(rb_doc)?.data().content.get() {
        Some(Body::Html(c)) => c.is_borrowed(),
        Some(Body::Xml(c)) => c.is_borrowed(),
        None => false,
    })
}

/// `Err(TypeError)` unless `rb_doc` is an HTML Document.
pub fn html_doc_check(rb_doc: Value) -> Result<(), Error> {
    HTML_DOC_TYPE.ensure(rb_doc)
}

/// Run `f` over the parsed document of a VALUE known to be an HTML Document,
/// for a read confined to `f` - which must run no Ruby. An index a reader
/// builds is a cell (`HtmlParsed`), so readers take the document shared and
/// may overlap.
pub(in crate::bridge) fn with_html_parsed_known<R>(
    rb_doc: Value,
    f: impl FnOnce(&HtmlParsed) -> R,
) -> Result<R, Error> {
    let doc = doc_known(rb_doc);
    let read = html_read(&doc)?;
    Ok(f(read.parsed()))
}

/// [`with_html_parsed_known`] for a Document of either kind: `None` for an
/// XML one.
pub(in crate::bridge) fn with_html_parsed_if<R>(
    rb_doc: Value,
    f: impl FnOnce(&HtmlParsed) -> R,
) -> Result<Option<R>, Error> {
    let doc = doc_known(rb_doc);
    match body_known(&doc)? {
        Body::Html(c) => {
            let read = HtmlRead::of(c).map_err(held_error)?;
            Ok(Some(f(read.parsed())))
        }
        Body::Xml(_) => Ok(None),
    }
}

/// The hidden ivar a Document keeps its parse-warning Array in.
static ERRORS: HiddenIvar = HiddenIvar::new(c"makiri_errors");

/// The name of [`ERRORS`].
pub(in crate::bridge) fn ivar_errors() -> crate::bridge::rb::HiddenId {
    ERRORS.id()
}

/// Intern the Document's hidden ivar's name. From `Init_makiri`, before any
/// method that reads it is defined.
pub fn init_ivars(ruby: &Ruby) -> Result<(), Error> {
    ERRORS.intern(ruby)
}

/// A Document's wrapper data, for the fields that are the wrapper's own
/// rather than the content's - every one of them a cell, so this is shared.
pub(super) fn doc_data_known(doc: &DocHandle) -> &DocData {
    doc.data()
}
