//! The Ruby wrappers of Makiri's nodes and Documents: the structs behind
//! them, their TypedData types and GC hooks, the one way a Document gets its
//! parsed handle ([`DocumentShell`]), and the representation-agnostic
//! accessors every node method starts from.
//!
//! Which representation a wrapper holds is decided here, by its TypedData
//! type; the HTML and XML front doors built on it are `bridge::html` and
//! `bridge::xml`.

#![allow(unsafe_code)]

use core::ptr::NonNull;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{Error, Value};

use crate::bridge::ruby::{value, VALUE};
use crate::bridge::typed::{Hooks, Marker, Relocator, TypedType};
use crate::falloc::{MapInsert, Reserve};
use crate::init::{RbConst, CLASS_DOCUMENT};
use crate::lexbor::adapter::html::{HtmlDoc, HtmlNodeKey, RawDoc, RawNode};
use crate::lexbor::adapter::post_parse::HtmlParsed;
use crate::node_type::NodeType;
use crate::token::{Kind, Token};
use crate::xml::model::{Document as XmlDoc, NodeId};
use core::hash::BuildHasherDefault;
use std::collections::HashMap;

/// The placeholder a wrapper's VALUE field holds until `TypedType::wrap`'s
/// store step writes the real one: `Qfalse`, which the mark ignores.
const QFALSE: VALUE = rb_sys::Qfalse as VALUE;

/* ------------------------------------------------------------------ *
 * the node wrapper                                                   *
 * ------------------------------------------------------------------ */

/// A node as the Ruby layer stores it: one pointer-sized word that is a Lexbor
/// node pointer in an HTML document and an arena [`NodeId`] in an XML one.
///
/// Which of the two it is is not stored beside it. It is the document's
/// representation - the wrapper's TypedData type, a NodeSet's [`DocKind`] -
/// decided once, so a word is read back only through the reader named for
/// that kind ([`xml`](Self::xml), [`html`](Self::html), [`token`](Self::token)).
/// This type is where the word crosses between the typed forms and the
/// untyped one, and so the one place in the glue a stored node is cast.
///
/// `repr(transparent)` over `usize`: a NodeSet's buffer keeps the layout it had
/// as `*mut c_void`. (A [`NodeData`] does not use it any more - it stores an
/// opaque [`NodeHandle`], which is larger by design.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct NodeWord(usize);

impl From<NodeId> for NodeWord {
    #[inline]
    fn from(id: NodeId) -> Self {
        NodeWord(id.to_token())
    }
}

impl From<RawNode> for NodeWord {
    #[inline]
    fn from(n: RawNode) -> Self {
        NodeWord(n.as_ptr() as usize)
    }
}

impl NodeWord {
    /// The word of an engine token. The token's kind is dropped: the word goes
    /// into a set or wrapper whose document already names it.
    #[inline]
    pub fn of_token(t: Token) -> Self {
        NodeWord(t.word())
    }

    /// The XML node, or `None` for a word naming no slot (a null one) - the
    /// same shape as [`html`](Self::html). Safe for any word: an arena reads a
    /// `NodeId` through `Document::try_node`, which rejects a stale or foreign
    /// one.
    #[inline]
    pub fn xml(self) -> Option<NodeId> {
        NodeId::from_token(self.0)
    }

    /// The HTML node; `None` for a null word.
    ///
    /// # Safety
    /// The word was made from a [`RawNode`] (it belongs to an HTML document),
    /// and that document is still alive - a `RawNode` is trusted to be live.
    #[inline]
    pub unsafe fn html(self) -> Option<RawNode> {
        RawNode::from_ptr(self.0 as *mut core::ffi::c_void)
    }

    /// The engine token for this node of a document of kind `kind`. Infallible:
    /// a [`DocKind`] is HTML or XML, never the token table's null slot, so the
    /// one place a Ruby-held node becomes a token cannot mint a null kind.
    ///
    /// # Safety
    /// The word is a live node of a document of kind `kind`.
    #[inline]
    pub unsafe fn token(self, kind: DocKind) -> Token {
        match kind {
            DocKind::Html => Token::html(self.0 as *mut core::ffi::c_void),
            DocKind::Xml => Token::xml(self.0),
        }
    }

    /// Node identity as an integer: for `#==`/`#hash` and the wrapper cache's
    /// key. Never read back as a node.
    #[inline]
    pub fn identity(self) -> usize {
        self.0
    }
}

/// A node as a long-lived Ruby wrapper stores it: the opaque [`HtmlNodeKey`]
/// for HTML, the arena [`NodeId`] for XML.
///
/// The HTML variant is what makes the stored handle opaque: it is a private
/// mint (see `lexbor::adapter`), so a wrapper's node can only have come from
/// the document the wrapper marks. The XML variant carries its document stamp
/// for the same reason. Which variant a wrapper holds is decided by its
/// TypedData type, as before.
#[derive(Clone, Copy)]
pub enum NodeHandle {
    Html(HtmlNodeKey),
    Xml(NodeId),
}

/// A live representation-specific node that can become a long-lived wrapper
/// handle. Its cache identity and stored handle are derived from the same
/// value, so callers cannot accidentally pair an unrelated token and handle.
pub(in crate::bridge) trait NodeHandleSource {
    fn identity(&self) -> usize;
    fn into_handle(self, document: Value) -> NodeHandle;
}

impl NodeHandle {
    /// The HTML key, when this is an HTML node.
    #[inline]
    pub fn html(self) -> Option<HtmlNodeKey> {
        match self {
            NodeHandle::Html(key) => Some(key),
            NodeHandle::Xml(_) => None,
        }
    }

    /// The XML id, when this is an XML node.
    #[inline]
    pub fn xml(self) -> Option<NodeId> {
        match self {
            NodeHandle::Xml(id) => Some(id),
            NodeHandle::Html(_) => None,
        }
    }
}

/// A node wrapper's data: the node plus the keepalive Document.
///
/// The node is owned by the document's arena (HTML or XML), so the wrapper
/// never frees it; the Document reference is what keeps it alive, and marking
/// it is the wrapper's whole GC job.
pub struct NodeData {
    /// Representation-opaque; read it only through a kind-checked accessor.
    pub node: NodeHandle,
    pub document: VALUE,
}

/* Measured: `NodeHandle` fits two words (the `NonNull`/`NonZero` niches carry
 * the discriminant), so `NodeData` is three words - one more than the two-word
 * `NodeWord` + `VALUE` it replaced. Pinned as a growth ratchet: a field that
 * pushes wrapper data past three words fails the build. */
const _: () = assert!(
    core::mem::size_of::<NodeData>() <= 24,
    "NodeData grew past three words; re-measure the wrapper cost before raising this"
);

impl Hooks for NodeData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.document);
    }
}

pub static NODE_DATA_TYPE: TypedType<NodeData> = TypedType::base(c"Makiri::Node".as_ptr());

pub static HTML_NODE_TYPE: TypedType<NodeData> =
    TypedType::derived(c"Makiri::HTML::Node".as_ptr(), &NODE_DATA_TYPE);

pub static XML_NODE_TYPE: TypedType<NodeData> =
    TypedType::derived(c"Makiri::XML::Node".as_ptr(), &NODE_DATA_TYPE);

/// Which representation a wrapped Ruby node is, decided by its TypedData type
/// rather than its Ruby class. A Document, a NodeSet or any non-node is
/// `Other`.
///
/// An enum, not the C's `c_int` codes: a transcribed code (`NODE_KIND_XML = 1`
/// where it was 2) once made `Document#import_node` read every HTML node as an
/// XML one, and copies of those numbers had spread to three files. A `match`
/// over this is checked for exhaustiveness and cannot be off by one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeRepr {
    Html,
    Xml,
    Other,
}

/* ------------------------------------------------------------------ *
 * the document wrapper                                               *
 * ------------------------------------------------------------------ */

/// What a Document owns: its parsed content, in one of the two
/// representations.
///
/// Raw pointers, from `Box::leak`, freed by `DocData`'s `Drop`. Not `Box`,
/// deliberately: readers copy the pointer out of a `&DocData` and hold what
/// they derive from it well past that borrow - an XPath context keeps this
/// pointer for the Document's whole life and reborrows the content per
/// evaluate, while other calls take `&mut DocData` for the node cache. A `Box` field
/// would assert unique ownership underneath those live aliases (a Stacked
/// Borrows hazard); a `NonNull` asserts nothing. `Copy`, so a reader gets the
/// pointer without borrowing the wrapper.
#[derive(Clone, Copy)]
pub(in crate::bridge) enum Content {
    /// Not yet installed - only between `DocumentShell::new` and `install`.
    Empty,
    Html(NonNull<HtmlParsed>),
    Xml(NonNull<XmlDoc>),
}

impl Content {
    /// The HTML document, when this is one.
    pub(in crate::bridge) fn html(self) -> Option<NonNull<HtmlParsed>> {
        match self {
            Content::Html(p) => Some(p),
            _ => None,
        }
    }
}

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
struct NodeCache {
    /// Empty until the first navigation, so a document nobody walks pays
    /// nothing.
    /// Tokens are a slot index or an aligned pointer - already well
    /// distributed - so they are mixed ([`crate::ptr_table::MixHasher`]) rather
    /// than hashed.
    map: HashMap<usize, VALUE, BuildHasherDefault<crate::ptr_table::MixHasher>>,
}

impl NodeCache {
    fn get(&self, token: usize) -> Option<VALUE> {
        self.map.get(&token).copied()
    }

    /// Room for one more wrapper, so the [`NodeCache::insert`] after it
    /// cannot fail - see [`DocData::reserve_node`].
    fn reserve(&mut self) -> Result<(), ()> {
        self.map.falloc_reserve(1)
    }

    /// Remember `wrapper` as the one wrapper for `token`. After
    /// [`NodeCache::reserve`] this allocates nothing, so it does not fail.
    fn insert(&mut self, token: usize, wrapper: VALUE) -> Result<(), ()> {
        self.map.falloc_insert(token, wrapper)
    }

    /// MOVABLE, not pinned: a document walked end to end holds one entry per
    /// node, and pinning them all would stop compaction doing its job. Paired
    /// with [`NodeCache::compact`].
    fn mark(&self, marker: &Marker) {
        for v in self.map.values() {
            marker.mark_movable(*v);
        }
    }

    fn compact(&mut self, relocator: &Relocator) {
        for v in self.map.values_mut() {
            *v = relocator.location(*v);
        }
    }

    fn memsize(&self) -> usize {
        self.map.capacity() * (core::mem::size_of::<usize>() + core::mem::size_of::<VALUE>())
    }
}

/// A Document's edit bookkeeping: the mutation gate an XPath evaluation with
/// a handler holds closed, and the versions readers key their caches by. Kept
/// together because they answer one question - may this document change, and
/// has it - and only [`record_edit`] and [`DocumentEvaluation`] write them.
#[derive(Default)]
struct EditState {
    /// How many XPath evaluations that can run Ruby (ones with a handler) are
    /// reading this document right now. Every mutator refuses while it is
    /// non-zero - see [`DocumentEvaluation`].
    evaluating: usize,
    /// `Document#tree_version`: the child-list edits ([`EditKind::ChildList`]).
    tree_version: u64,
    /// `Document#attribute_version`: the attribute edits
    /// ([`EditKind::Attributes`]).
    attribute_version: u64,
}

impl EditState {
    /// Count `kind`'s edit by the version that counts it - by at most one, so
    /// a reader keying a cache by one version is not refilled for another's
    /// edits.
    fn record(&mut self, kind: EditKind) {
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
#[derive(Default)]
struct ExternalReport {
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
    fn update(&mut self, now: usize, chunks: usize) -> Option<isize> {
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
pub struct DocData {
    /// Set once, by `DocumentShell::install`; read through the accessors below.
    content: Content,
    /// The mutation gate and the edit versions ([`EditState`]).
    edits: EditState,
    errors: VALUE,
    /// What the GC has been told about the content ([`ExternalReport`]).
    report: ExternalReport,
    /// One wrapper per node; see [`NodeCache`].
    ///
    /// Boxed and optional so a document nobody navigates never allocates a
    /// cache, and its wrapper stays one pointer wide.
    nodes: Option<Box<NodeCache>>,
}

impl DocData {
    /// The one wrapper for `token`, or None until something navigates to it.
    fn cached(&self, token: usize) -> Option<VALUE> {
        self.nodes.as_ref()?.get(token)
    }

    /// Make room in the cache (allocating it on first use) for the wrapper
    /// about to be built. This is the step that can fail, and it comes
    /// BEFORE the wrapper exists: a wrapper made and then not cached would be
    /// a second object for its node on the next navigation, without the
    /// first one's `freeze`, instance variables or singleton methods.
    fn reserve_node(&mut self) -> Result<(), ()> {
        if self.nodes.is_none() {
            self.nodes = Some(
                crate::falloc::try_box(NodeCache {
                    map: HashMap::with_hasher(BuildHasherDefault::default()),
                })
                .map_err(|_| ())?,
            );
        }
        self.nodes.as_mut().ok_or(())?.reserve()
    }

    /// Remember `wrapper` for `token`, in the room [`DocData::reserve_node`]
    /// made.
    fn cache(&mut self, token: usize, wrapper: VALUE) -> Result<(), ()> {
        self.nodes.as_mut().ok_or(())?.insert(token, wrapper)
    }

    /// The Document's parse-warning Array.
    pub fn errors(&self) -> Value {
        // SAFETY: the live Array this wrapper marks.
        unsafe { value(self.errors) }
    }

    /// Whether the content has grown enough since the last report to measure
    /// and report it again ([`account_growth`]): by an eighth, and at least
    /// [`GROWTH_MIN_CHUNKS`] chunks (HTML) or [`GROWTH_MIN_BYTES`] (XML).
    fn grown_enough(&self) -> bool {
        // SAFETY: the content is owned by this object and live for the call.
        unsafe {
            match self.content {
                Content::Empty => false,
                Content::Html(p) => {
                    let then = self.report.chunks;
                    p.as_ref().arena_chunks()
                        >= then.saturating_add((then / 8).max(GROWTH_MIN_CHUNKS))
                }
                Content::Xml(d) => {
                    let then = self.report.bytes;
                    d.as_ref().memsize() >= then.saturating_add((then / 8).max(GROWTH_MIN_BYTES))
                }
            }
        }
    }

    /// The HTML content's chunk count, 0 for XML.
    fn arena_chunks(&self) -> usize {
        // SAFETY: as `grown_enough`.
        unsafe { self.content.html().map_or(0, |p| p.as_ref().arena_chunks()) }
    }

    /// The bytes the content holds outside Ruby's allocator, or 0 with none.
    fn external_bytes(&self) -> usize {
        // SAFETY: the content is owned by this object and live for the call.
        unsafe {
            match self.content {
                Content::Empty => 0,
                Content::Html(p) => p.as_ref().external_bytes(),
                Content::Xml(d) => d.as_ref().memsize(),
            }
        }
    }
}

impl Hooks for DocData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.errors);
        if let Some(cache) = self.nodes.as_ref() {
            cache.mark(marker);
        }
    }

    fn compact(&mut self, relocator: &Relocator) {
        if let Some(cache) = self.nodes.as_mut() {
            cache.compact(relocator);
        }
    }

    fn memsize(&self) -> usize {
        core::mem::size_of::<DocData>()
            .saturating_add(self.external_bytes())
            .saturating_add(self.nodes.as_ref().map_or(0, |c| c.memsize()))
    }
}

/// Run by the free callback, in Ruby's collector - so it must not panic (the
/// callback aborts rather than unwind into C) and must touch no Ruby object.
/// A `DocData` exists only in the memory `TypedType::wrap` gave it, so this
/// is the one place a Document's content is freed; the node cache (a `Box`
/// field) follows by drop glue.
impl Drop for DocData {
    fn drop(&mut self) {
        // SAFETY: each pointer came from `Box::leak` in `DocumentShell` and only
        // this wrapper owns it; readers' borrows are confined to calls on a
        // live Document, and this one is being collected.
        unsafe {
            match self.content {
                Content::Empty => {}
                Content::Html(p) => drop(Box::from_raw(p.as_ptr())),
                Content::Xml(d) => drop(Box::from_raw(d.as_ptr())),
            }
        }
        self.content = Content::Empty;
        /* Balance the report, or the GC keeps counting freed arenas as live
         * and collects ever more eagerly. A plain C call, as this hook has to
         * be: it only subtracts, and Ruby's own `xfree` does the same from
         * here. */
        if let Some(diff) = self.report.release() {
            crate::bridge::ruby::report_external_bytes(diff);
        }
    }
}

/// Tell the GC how much memory `rb_doc` holds outside Ruby's allocator.
///
/// Neither the Lexbor arena nor the XML arena is an `xmalloc`, so the GC sees
/// a parsed document as a few dozen bytes: a loop that parses and drops never
/// triggers a collection from memory pressure, RSS climbs by a document per
/// parse, and every parse pays for freshly faulted pages (measured: 2× the
/// parse time, and gigabytes of RSS, on a 280 KB document). This reports the
/// difference since the last call, so it is safe to call again after the
/// document grows.
///
/// May run a collection right here, so `rb_doc` must be reachable from the
/// caller's frame (a local VALUE is), and nothing borrowed from a Ruby String
/// may be held across the call.
fn account_document(rb_doc: VALUE) {
    // SAFETY: `rb_doc` is a Document (the base type matches either leaf).
    let d = unsafe { &mut *(DOC_TYPE.known_ptr(value(rb_doc))) };
    let (now, chunks) = (d.external_bytes(), d.arena_chunks());
    if let Some(diff) = d.report.update(now, chunks) {
        crate::bridge::ruby::report_external_bytes(diff);
    }
}

/// Chunks an HTML document's pools must gain before [`account_growth`]
/// measures it again (Lexbor's node pool grows in chunks of tens of KiB).
const GROWTH_MIN_CHUNKS: usize = 16;
/// Bytes an XML document must gain before [`account_growth`] reports again.
const GROWTH_MIN_BYTES: usize = 512 * 1024;

/// Tell the GC about what `rb_doc` has grown by since its last report, once
/// that is enough to matter ([`DocData::grown_enough`]: an eighth, and a floor).
///
/// A parse reports its document's size once; mutation grows the arena after
/// that - nodes appended in a loop, `inner_html=` of a large string - and
/// without a new report the GC keeps judging a large document by its size at
/// parse time. Measuring an HTML document walks every chunk of its pools, too
/// costly for every edit, so an O(1) chunk count decides when; growth by a
/// fraction also keeps the measurements logarithmic in the final size.
///
/// Called where an edit begins (`bridge::html::edit`, `bridge::xml::
/// begin_edit`): the growth it sees is the previous edits', and a document's
/// last edit is reported at its next one or not at all - an under-report, the
/// safe direction, as it was before this existed. It may run a collection,
/// so, like [`account_document`], it must be called with `rb_doc` on the
/// caller's stack and no borrowed Ruby String held - which is so at the start
/// of an edit, before any argument is converted.
pub fn account_growth(rb_doc: Value) {
    if with_doc_data_known(rb_doc, |d| d.grown_enough()) {
        account_document(rb_doc.as_raw());
    }
}

/// The base type: the kind-agnostic accessors (`doc_content`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: TypedType<DocData> = TypedType::base(c"Makiri::Document".as_ptr());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
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
            DocKind::Html => Kind::Html,
            DocKind::Xml => Kind::Xml,
        }
    }
}

/// A Document wrapper allocated before the content it will own, and the only
/// way a Document gets that content.
///
/// The order is the point. Allocating the Ruby wrapper can raise
/// (`NoMemoryError`), and a raise `longjmp`s past Rust destructors, so a parse
/// result held at that moment would leak. So the wrapper is made FIRST - while
/// nothing needs freeing - and the parsed handle goes in afterwards through
/// [`install`](Self::install), which also reports the arena to the GC. That
/// report is not optional (see `account_document`), and the parse entries used
/// to make it by hand, each one a place to forget it.
pub struct DocumentShell(VALUE);

impl DocumentShell {
    pub fn new(kind: DocKind) -> DocumentShell {
        let (klass, ty) = match kind {
            DocKind::Html => (crate::init::CLASS_HTML_DOCUMENT.raw(), &HTML_DOC_TYPE),
            DocKind::Xml => (crate::init::CLASS_XML_DOCUMENT.raw(), &XML_DOC_TYPE),
        };
        /* The errors array is built before the wrap and kept in a local, so the
         * conservative stack scan pins it across the wrap's allocation and the
         * store closure does not allocate (an allocation there could raise
         * NoMemoryError while the caller holds a live resource). */
        let errors = crate::bridge::ruby::array_new();
        // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
        DocumentShell(unsafe {
            ty.wrap(
                klass,
                || DocData {
                    content: Content::Empty,
                    edits: EditState::default(),
                    errors: QFALSE,
                    report: ExternalReport::default(),
                    nodes: None,
                },
                |d| d.errors = errors.as_raw(),
            )
        })
    }

    /// Give the HTML Document its parsed document - the GC owns it from here -
    /// and report the arena's size.
    pub fn install_html(self, parsed: Box<HtmlParsed>) -> Value {
        self.install(Content::Html(NonNull::from(Box::leak(parsed))))
    }

    /// Give the XML Document its arena, as [`install_html`](Self::install_html).
    pub fn install_xml(self, arena: Box<XmlDoc>) -> Value {
        self.install(Content::Xml(NonNull::from(Box::leak(arena))))
    }

    fn install(self, content: Content) -> Value {
        // SAFETY: `self.0` is a Document wrapper (the base type matches either
        // leaf) that has no content yet, so nothing is overwritten.
        unsafe {
            let d = DOC_TYPE.known_ptr(value(self.0));
            (*d).content = content;
        }
        account_document(self.0);
        // SAFETY: a live Document.
        unsafe { value(self.0) }
    }
}

/* ------------------------------------------------------------------ *
 * lexbor <-> wrapper accessors                                       *
 * ------------------------------------------------------------------ */

/// The content of any Document. `Err(TypeError)` for a non-Document.
pub(in crate::bridge) fn doc_content(rb_doc: Value) -> Result<Content, Error> {
    Ok(DOC_TYPE.get(&rb_doc)?.content)
}

/// [`doc_content`] for a VALUE already known to be a Document - a node's
/// keepalive Document, or the receiver of a Document method.
pub(in crate::bridge) fn doc_content_known(rb_doc: Value) -> Content {
    DOC_TYPE.get_known(&rb_doc).content
}

/// The XML arena of a Document, or null when it is not an XML one.
pub(in crate::bridge) fn xml_arena_known(rb_doc: Value) -> *mut XmlDoc {
    match doc_content_known(rb_doc) {
        Content::Xml(d) => d.as_ptr(),
        _ => core::ptr::null_mut(),
    }
}

/// The Lexbor document behind an HTML Document. `Err(TypeError)` otherwise.
pub fn html_doc_unwrap(rb_doc: Value) -> Result<RawDoc, Error> {
    let d: &DocData = HTML_DOC_TYPE.get(&rb_doc)?;
    Ok(html_doc_of(d))
}

/// The Lexbor document of `rb_doc`, a VALUE already known to be an HTML
/// Document (a Document method's receiver), borrowed for as long as the caller
/// borrows it.
pub fn html_doc(rb_doc: &Value) -> HtmlDoc<'_> {
    // SAFETY: a live HTML Document, kept alive by `rb_doc`, which the caller
    // holds for the borrow.
    unsafe { html_doc_known(*rb_doc).as_doc() }
}

/// [`html_doc_unwrap`] for a VALUE already known to be an HTML Document.
pub fn html_doc_known(rb_doc: Value) -> RawDoc {
    html_doc_of(HTML_DOC_TYPE.get_known(&rb_doc))
}

#[allow(
    clippy::expect_used,
    reason = "an HTML Document holds HTML content from `install` on, and none is reachable before"
)]
fn html_doc_of(d: &DocData) -> RawDoc {
    /* The HTML type guarantees HTML content once installed, and nothing
     * reaches a Document before `install`: a broken invariant. */
    let p = d
        .content
        .html()
        .expect("an HTML Document without its document");
    /* An lxb_html_document_t leads with its lxb_dom_document_t, so this is a
     * downcast to the embedded base, not a reinterpretation. */
    // SAFETY: the live document this Document owns.
    unsafe { p.as_ref().raw_doc() }
}

/// Run `f` over the parsed document of a VALUE known to be an HTML Document.
///
/// The `&mut HtmlParsed` does not escape `f`, so the raw pointer stays in this
/// layer and no alias can outlive the call. `f` must not run Ruby that could
/// re-enter this document (the readers' closures copy, they do not call back).
#[allow(
    clippy::expect_used,
    reason = "an HTML Document holds HTML content from `install` on, and none is reachable before"
)]
pub(in crate::bridge) fn with_html_parsed_known<R>(
    rb_doc: Value,
    f: impl FnOnce(&mut HtmlParsed) -> R,
) -> R {
    let mut p = doc_content_known(rb_doc)
        .html()
        .expect("an HTML Document without its document");
    // SAFETY: under the GVL, and the borrow is confined to `f`.
    unsafe { f(p.as_mut()) }
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
    if let Some(cached) = cached_node(document, token) {
        return Ok(cached);
    }
    /* Room for the entry first: a failure after the wrap would hand out a
     * wrapper the cache does not know (DocData::reserve_node). */
    with_doc_data_known(document, DocData::reserve_node).map_err(|_| wrap_oom())?;
    let node = source.into_handle(document);
    /* The Document is stored after the wrap: see `TypedType::wrap`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    let fresh = unsafe {
        value(ty.wrap(
            klass,
            || NodeData {
                node,
                document: QFALSE,
            },
            |nd| nd.document = document.as_raw(),
        ))
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
fn cached_node(rb_doc: Value, token: usize) -> Option<Value> {
    // SAFETY: as `with_doc_data_known`.
    let v = with_doc_data_known(rb_doc, |d| d.cached(token));
    // SAFETY: a VALUE this document marks, so it is live.
    v.map(|v| unsafe { value(v) })
}

/// `FrozenError` when the node `source` names has a wrapper and that wrapper is
/// frozen. A node with no wrapper yet cannot have been frozen. For an edit
/// that reaches a node other than its receiver - an Attr's owner element,
/// whose attribute list an Attr's `remove` or `content=` changes.
pub(in crate::bridge) fn check_node_frozen(
    rb_doc: Value,
    source: impl NodeHandleSource,
) -> Result<(), Error> {
    match cached_node(rb_doc, source.identity()) {
        Some(wrapper) => crate::bridge::ruby::check_frozen(wrapper),
        None => Ok(()),
    }
}

/// Remember `wrapper` as the one wrapper for `token` under `rb_doc`.
fn cache_node(rb_doc: Value, token: usize, wrapper: Value) -> Result<(), ()> {
    with_doc_data_known(rb_doc, |d| d.cache(token, wrapper.as_raw()))
}

/// Run `f` over a Document's wrapper data, for the fields that are the
/// wrapper's own rather than the content's (the evaluation count).
fn with_doc_data_known<R>(rb_doc: Value, f: impl FnOnce(&mut DocData) -> R) -> R {
    // SAFETY: a live Document (the base type matches either leaf), under the
    // GVL, with the borrow confined to `f`.
    unsafe { f(&mut *DOC_TYPE.known_ptr(rb_doc)) }
}

/// Why [`node_token_in`] refused a node.
pub enum NotInDocument {
    /// Not a usable Makiri node - the `TypeError` or `Makiri::Error` its
    /// wrapper raised.
    Unusable(Error),
    /// A node of another document.
    Foreign,
}

/// The engine token of `rb_node`, which must be a node of `document` (the
/// document node included).
///
/// The check behind every token minted for one document's engine - a handler's
/// result node, an `XPathContext`'s context node - made here once, so the
/// `unsafe` mint never rests on a caller having made it. The token's kind is
/// `document`'s own, not one a caller passes alongside.
pub fn node_token_in(rb_node: Value, document: Value) -> Result<Token, NotInDocument> {
    let node_document = keepalive_document(rb_node).map_err(NotInDocument::Unusable)?;
    if node_document.as_raw() != document.as_raw() {
        return Err(NotInDocument::Foreign);
    }
    let raw = node_raw(rb_node).map_err(NotInDocument::Unusable)?;
    // SAFETY: a live node of `document` - its wrapper holds that document -
    // minted for `document`'s own kind.
    Ok(unsafe { raw.token(DocKind::of(document)) })
}

/// The kind-AGNOSTIC node word (the base type, so HTML or XML). Only for the
/// few sites where the representation is irrelevant (identity comparison) or
/// already guaranteed by an external same-document check (the XPath context
/// node, a NodeSet of the node's own document).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to Lexbor's.
pub fn node_raw(rb_node: Value) -> Result<NodeWord, Error> {
    if crate::bridge::ruby::is_kind_of(rb_node, &CLASS_DOCUMENT) {
        if let Content::Xml(xdoc) = doc_content(rb_node)? {
            // SAFETY: a Document's arena lives as long as the Document, and its
            // document node is read, not written.
            return Ok(unsafe { xdoc.as_ref() }.doc_node().into());
        }
        return Ok(RawNode::from(html_doc_unwrap(rb_node)?).into());
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    let nd: &NodeData = NODE_DATA_TYPE.get(&rb_node)?;
    Ok(match nd.node {
        NodeHandle::Html(key) => NodeWord::from(key.raw_node()),
        NodeHandle::Xml(id) => NodeWord::from(id),
    })
}

/// Which representation `v` wraps ([`NodeRepr`]).
pub fn node_repr(v: Value) -> NodeRepr {
    if HTML_NODE_TYPE.is(v) {
        NodeRepr::Html
    } else if XML_NODE_TYPE.is(v) {
        NodeRepr::Xml
    } else {
        NodeRepr::Other
    }
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub fn node_identity(rb_node: Value) -> Result<usize, Error> {
    Ok(node_raw(rb_node)?.identity())
}

/// Node identity for `==`/`eql?`: the representation AND the word. The word
/// alone does not tell the two apart - an XML node's is its arena index packed
/// with its document's stamp, not an address, so it can equal a live HTML
/// node's pointer.
pub fn node_key(rb_node: Value) -> Result<(DocKind, usize), Error> {
    Ok((
        DocKind::of(keepalive_document(rb_node)?),
        node_identity(rb_node)?,
    ))
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub fn keepalive_document(rb_node: Value) -> Result<Value, Error> {
    if crate::bridge::ruby::is_kind_of(rb_node, &CLASS_DOCUMENT) {
        return Ok(rb_node);
    }
    let nd: &NodeData = NODE_DATA_TYPE.get(&rb_node)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
}

/* ---- the edit versions ---- */

/// What an edit changes, and so which version counts it ([`record_edit`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    /// A child list of a node the document owns - attached, detached or inside
    /// a fragment alike: `Document#tree_version`.
    ChildList,
    /// An attribute of an element the document owns - added, removed, its
    /// value set (to the same value too) - or an Attr's value:
    /// `Document#attribute_version`.
    Attributes,
    /// A Text, Comment, CDATA or PI node's data. No version counts it: no
    /// reader has asked for one, and `tree_version` counting it made every
    /// keystroke in a text field refill every child-list cache.
    CharacterData,
}

/// Record an edit of `kind` to a node `rb_doc` owns: the one way a version
/// moves. Called where an edit is handed its mutable node (`HtmlEdit`,
/// `Editing`) and for the source document of an adoption, so no mutator can
/// miss it. Dropping what the edit invalidates is the representation's own
/// step, taken beside this one: an HTML document's indexes
/// ([`invalidate_indexes`]), the XML arena's name index (the arena's).
///
/// The versions are INVALIDATION keys, not edit counts. The contract, for HTML
/// and XML alike:
///
/// - an unchanged version means the data it keys did not change -
///   `tree_version` the child lists, `attribute_version` the attributes;
/// - a changed one may be conservative: an edit that then fails, or stops
///   part-way, still moved it;
/// - it is recorded once the edit's refusals are past - the frozen check, the
///   evaluation guard - and its arguments converted, immediately BEFORE the
///   change, with the representation's indexes dropped at the same point;
/// - no Ruby runs between the record and the change, so nothing can read the
///   tree in between and cache it under the new number.
///
/// Recording after the change instead would miss an edit that panics, or
/// fails having changed something, and leave a reader trusting a stale cache.
pub fn record_edit(rb_doc: Value, kind: EditKind) {
    with_doc_data_known(rb_doc, |d| d.edits.record(kind));
}

/// `Document#tree_version`: how many child-list edits the document has seen.
/// `TypeError` for a non-Document.
pub fn tree_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.get(&rb_doc)?.edits.tree_version)
}

/// `Document#attribute_version`: how many attribute edits the document has
/// seen. `TypeError` for a non-Document.
pub fn attribute_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.get(&rb_doc)?.edits.attribute_version)
}

/* ---- the document's mutation gate ---- */

/// `Err(Makiri::Error)` while an evaluation with a handler is reading `rb_doc`.
/// Every mutator checks this before it changes anything.
pub fn ensure_document_mutable(rb_doc: Value) -> Result<(), Error> {
    if with_doc_data_known(rb_doc, |d| d.edits.evaluating) != 0 {
        return Err(makiri_error(
            "cannot modify a document while evaluating XPath over it (re-entrant mutation from a handler)",
        ));
    }
    Ok(())
}

/// Drop the DOM and text indexes so the next query rebuilds them. An XML
/// Document keeps none.
pub fn invalidate_indexes(rb_doc: Value) {
    if let Content::Html(_) = doc_content_known(rb_doc) {
        with_html_parsed_known(rb_doc, HtmlParsed::invalidate_indexes);
    }
}

/* ------------------------------------------------------------------ *
 * the evaluation guard                                                *
 * ------------------------------------------------------------------ */

/// Marks a document as read by an XPath evaluation that can run Ruby - one with
/// a handler - for as long as it lives. Nested evaluations stack.
///
/// The engine borrows names, attribute values and index slices out of the
/// document for the whole walk, and a handler runs arbitrary Ruby in the middle
/// of it. Lexbor frees an attribute's old value when a new one is set
/// (`lxb_dom_attr_set_value`), and a mutation drops the indexes, so a handler
/// that edited the same document could leave the evaluator reading freed
/// memory. Every mutator checks [`ensure_document_mutable`]
/// first, so that borrow is never invalidated under a suspended walk.
pub struct DocumentEvaluation(
    /// The Document the count belongs to. Holding it is what keeps the parsed
    /// handle valid: a guard lives on the machine stack, which Ruby's collector
    /// scans, so the Document cannot be collected while one is alive.
    Value,
);

impl DocumentEvaluation {
    pub fn enter(rb_doc: Value) -> Result<Self, Error> {
        DOC_TYPE.get(&rb_doc)?; /* TypeError for a non-Document */
        with_doc_data_known(rb_doc, |d| d.edits.evaluating += 1);
        Ok(DocumentEvaluation(rb_doc))
    }
}

impl Drop for DocumentEvaluation {
    fn drop(&mut self) {
        with_doc_data_known(self.0, |d| d.edits.evaluating -= 1);
        /* Read the Document here, so the guard demonstrably holds it: the field
         * is there to keep it reachable, and a field nothing reads is one the
         * compiler is free to treat as absent. */
        core::hint::black_box(self.0);
    }
}
