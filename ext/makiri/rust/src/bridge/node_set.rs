//! `Makiri::NodeSet`.
//!
//! A NodeSet is an array of [`NodeWord`]s plus a keepalive reference to the
//! owning Document. The nodes are owned by the document's arena, so marking the
//! document keeps every one of them alive - that is the whole GC contract.
//!
//! The stored words are representation-opaque. The set never dereferences
//! one; it compares them for identity and, when vending a node, reads it back
//! as the representation named by its `kind`. That keeps an XML set from ever reading
//! an XML arena handle as an HTML one. The kind is decided once at
//! construction rather than probed per node, which would regress the hot
//! traversal path.
//!
//! # This wrapper type lives in the bridge
//!
//! It owns its `rb_data_type_t`, its GC `mark`/`size`/free, and a buffer in
//! Ruby's allocator (`alloc::RubyVec`), so its layout is private and it belongs
//! with the other raw-Ruby-ABI seams rather than in the glue. The glue builds a set through
//! `node_set_with_fill`, `node_set_from` or `node_set_of_nodes`, all over
//! the Document's opaque `VALUE`; the [`Fill`] handle carries the "this is a
//! NodeSet" invariant so its pushes are safe.
//!
//! Mutation goes through a `RefCell`, and every borrow failure becomes a Ruby
//! error rather than a panic: a panic would reach Ruby as `fatal` (the crate
//! unwinds, it no longer aborts), which is still the wrong answer for an
//! aliasing mistake a Ruby caller can provoke - this codebase fails closed by
//! raising an ordinary error.

#![allow(unsafe_code)]

use crate::bridge::alloc::{GrowError, RubyVec};
use crate::falloc::Reserve;
use std::cell::RefCell;
use std::collections::HashSet;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::value::Opaque;
use magnus::{gc::Marker, prelude::*, DataTypeFunctions, Error, Ruby, TypedData, Value};

use crate::bridge::node_wrap::wrap_doc_node;
use crate::bridge::typed::typed_data_unprotected;
use crate::bridge::wrapper::{keepalive_document, node_raw, DocKind, NodeWord};
use crate::init::{CLASS_DOCUMENT, CLASS_NODE, CLASS_NODE_SET};

use crate::limits::NODE_SET_MAX;

/// Below this operand size a linear scan beats building a hash set.
const HASH_MIN: usize = 64;

/// Is `v` an instance of `klass`?
use crate::bridge::ruby::is_kind_of;

/* ------------------------------------------------------------------ */
/* storage                                                            */
/* ------------------------------------------------------------------ */

/// How many nodes a set holds in itself before it takes an allocation: a
/// node's children, its ancestors and most query results on a small page fit,
/// so they cost no allocation (nor the protected call one takes) beyond the
/// set's own object.
const INLINE_NODES: usize = 8;

/// The set's node array, under the set's size cap: up to [`INLINE_NODES`] in
/// the set itself, and past that a [`RubyVec`] that holds them all.
struct NodeVec {
    inline: [NodeWord; INLINE_NODES],
    /// The nodes in `inline`; 0 once the set has spilled to `heap`.
    inline_len: usize,
    /// Every node once the set has spilled (its capacity is then non-zero);
    /// empty, holding no allocation, before.
    heap: RubyVec<NodeWord>,
}

/// Why a push was refused.
///
/// Deliberately small rather than a `magnus::Error`: a push runs once per node
/// of every result, and returning the large error type from each successful
/// push measured about 2ns a node. Callers convert through `From` on the failure path only.
#[derive(Clone, Copy)]
pub enum PushError {
    SizeLimit,
    CapacityOverflow,
    /// Ruby's allocator raised `NoMemoryError`, which this answers again.
    NoMemory,
    /// The set is borrowed elsewhere - a push from inside its own iteration.
    Busy,
}

impl PushError {
    fn message(self) -> String {
        match self {
            PushError::SizeLimit => {
                format!("node set size limit exceeded ({NODE_SET_MAX} nodes)")
            }
            PushError::CapacityOverflow => "node set capacity overflow".to_string(),
            PushError::NoMemory => "failed to allocate memory".to_string(),
            PushError::Busy => "node set is already in use".to_string(),
        }
    }
}

impl From<PushError> for Error {
    fn from(e: PushError) -> Error {
        match e {
            PushError::NoMemory => Error::new(
                crate::bridge::ruby::gvl_ruby().exception_no_mem_error(),
                e.message(),
            ),
            _ => makiri_error(e.message()),
        }
    }
}

impl From<GrowError> for PushError {
    fn from(e: GrowError) -> PushError {
        match e {
            GrowError::Overflow => PushError::CapacityOverflow,
            GrowError::NoMemory => PushError::NoMemory,
        }
    }
}

impl NodeVec {
    const fn new() -> Self {
        NodeVec {
            inline: [NodeWord::NULL; INLINE_NODES],
            inline_len: 0,
            heap: RubyVec::new(),
        }
    }

    fn spilled(&self) -> bool {
        self.heap.capacity() != 0
    }

    fn len(&self) -> usize {
        if self.spilled() {
            self.heap.len()
        } else {
            self.inline_len
        }
    }

    /// The nodes held outside the set's own memory, for its size report.
    fn capacity(&self) -> usize {
        self.heap.capacity()
    }

    fn as_slice(&self) -> &[NodeWord] {
        if self.spilled() {
            self.heap.as_slice()
        } else {
            &self.inline[..self.inline_len]
        }
    }

    /// Move to the heap with room for `n` nodes (at least the ones held), in
    /// one allocation.
    fn spill(&mut self, n: usize) -> Result<(), PushError> {
        self.heap.reserve(n.max(self.inline_len))?;
        for &w in &self.inline[..self.inline_len] {
            /* Room was reserved above, so this grows nothing. */
            self.heap.push(w)?;
        }
        self.inline_len = 0;
        Ok(())
    }

    /// Make room for `n` nodes (at most [`NODE_SET_MAX`]) in one allocation,
    /// so the pushes that follow cannot reallocate. Room that fits inline
    /// takes none.
    fn reserve(&mut self, n: usize) -> Result<(), PushError> {
        let n = n.min(NODE_SET_MAX);
        if self.spilled() {
            Ok(self.heap.reserve(n)?)
        } else if n > INLINE_NODES {
            self.spill(n)
        } else {
            Ok(())
        }
    }

    /// Append every node of `nodes`, as [`push`](Self::push) each: room for
    /// the iterator's lower bound is taken first, and a set on the heap is
    /// then filled in one loop, the size cap checked once for the whole run.
    fn extend(&mut self, nodes: impl Iterator<Item = NodeWord>) -> Result<(), PushError> {
        let mut nodes = nodes.peekable();
        self.reserve(self.len().saturating_add(nodes.size_hint().0))?;
        while nodes.peek().is_some() {
            if self.spilled() {
                let room = NODE_SET_MAX.saturating_sub(self.heap.len());
                for w in nodes.by_ref().take(room) {
                    self.heap.push(w)?;
                }
                if nodes.peek().is_some() {
                    return Err(PushError::SizeLimit);
                }
                return Ok(());
            }
            if let Some(w) = nodes.next() {
                self.push(w)?;
            }
        }
        Ok(())
    }

    /// Append one node: `Err` for the size cap, a capacity overflow or a
    /// failed allocation.
    #[inline]
    fn push(&mut self, node: NodeWord) -> Result<(), PushError> {
        if self.spilled() {
            if self.heap.len() >= NODE_SET_MAX {
                return Err(PushError::SizeLimit);
            }
            return Ok(self.heap.push(node)?);
        }
        if let Some(slot) = self.inline.get_mut(self.inline_len) {
            *slot = node;
            self.inline_len += 1;
            return Ok(());
        }
        /* Full inline: spill, with room to grow (NODE_SET_MAX is far above
         * INLINE_NODES, so the cap is not reached here). */
        self.spill(INLINE_NODES * 4)?;
        Ok(self.heap.push(node)?)
    }
}

/// Only the node array is mutable, and only it is behind the cell.
///
/// That split is load-bearing, not tidiness. `mark` must reach `document` on
/// every GC, and a GC can happen *while a push holds the mutable borrow* -
/// `ruby_xrealloc2` is an allocation, so it is a GC point. If `document` lived
/// inside the cell, `mark` would find it busy, fail to mark the Document, and
/// the arena holding every node in this set could be collected underneath it.
/// Fields that never change after construction belong outside the cell.
///
/// `TypedData` is implemented by hand rather than derived: the derive looks
/// the class up with a `funcall` of `const_get` (Ruby code, and a raise),
/// where `init` has the class already (`CLASS_NODE_SET`, whose allocator
/// `glue::node_set` undefines).
pub struct NodeSet {
    /// The owning Document. `Opaque` is magnus's form for a Ruby value stored
    /// in a Rust struct: it cannot be read back without a `&Ruby` (so never off
    /// a Ruby thread), and it is `Mark`, which is what makes storing it sound -
    /// `mark` below is what the GC follows to reach it.
    document: Opaque<Value>,
    /// Decided once: whether the stored words are XML arena handles (wrapped as
    /// `Makiri::XML::*`) or HTML arena nodes.
    kind: DocKind,
    nodes: RefCell<NodeVec>,
}

// SAFETY: `data_type` describes `NodeSet` - its mark, size and free are
// `DataTypeFunctions`' below, as the derive would build them - and `class`
// is the class `init` defined for it.
unsafe impl TypedData for NodeSet {
    fn class(_ruby: &Ruby) -> magnus::RClass {
        #[allow(
            clippy::expect_used,
            reason = "every NodeSet is made after Init_makiri"
        )]
        CLASS_NODE_SET
            .get()
            .expect("Makiri::NodeSet is defined at init")
    }

    fn data_type() -> &'static magnus::DataType {
        static DATA_TYPE: magnus::DataType = magnus::data_type_builder!(NodeSet, "Makiri::NodeSet")
            .mark()
            .size()
            .free_immediately()
            .build();
        &DATA_TYPE
    }
}

impl DataTypeFunctions for NodeSet {
    fn mark(&self, marker: &Marker) {
        /* No borrow: see the note on the struct. A GC can land mid-push. */
        marker.mark(self.document);
    }

    fn size(&self) -> usize {
        let base = core::mem::size_of::<Self>();
        /* Advisory only, so a busy cell just reports the header. */
        match self.nodes.try_borrow() {
            Ok(nodes) => base.saturating_add(
                nodes
                    .capacity()
                    .saturating_mul(core::mem::size_of::<NodeWord>()),
            ),
            Err(_) => base,
        }
    }
}

impl NodeSet {
    /// Borrow the contents, turning a borrow conflict into a Ruby error instead
    /// of a panic (which would surface as `fatal`).
    fn read(&self) -> Result<std::cell::Ref<'_, NodeVec>, Error> {
        self.nodes
            .try_borrow()
            .map_err(|_| makiri_error("node set is already in use"))
    }

    fn write(&self) -> Result<std::cell::RefMut<'_, NodeVec>, Error> {
        self.nodes
            .try_borrow_mut()
            .map_err(|_| makiri_error("node set is already in use"))
    }

    fn document(&self, ruby: &Ruby) -> Value {
        ruby.get_inner(self.document)
    }

    /// Append a node to a set this module built or checked; see [`Fill`]. `Err`
    /// only for the fail-closed refusals (size cap, capacity overflow, busy).
    #[inline]
    fn try_push(&self, node: NodeWord) -> Result<(), PushError> {
        let Ok(mut nodes) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        nodes.push(node)
    }

    /// [`NodeVec::extend`] on a set this module built or checked.
    fn try_extend(&self, nodes: impl Iterator<Item = NodeWord>) -> Result<(), PushError> {
        let Ok(mut set) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        set.extend(nodes)
    }

    /// Room for `n` more nodes in one allocation.
    fn try_reserve(&self, n: usize) -> Result<(), PushError> {
        let Ok(mut nodes) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        let want = nodes.len().saturating_add(n);
        nodes.reserve(want)
    }
}

/* ------------------------------------------------------------------ */
/* the constructors the glue calls                                    */
/* ------------------------------------------------------------------ */

/// An empty NodeSet over `document`, whose nodes it will hold.
///
/// The Document decides how a stored word is read back - an arena token for
/// XML, an HTML arena node for HTML - so it is checked here rather than taken on
/// trust: one class test per set, never per node.
pub fn node_set_new(document: Value) -> Result<Value, Error> {
    assert!(
        is_kind_of(document, &CLASS_DOCUMENT),
        "a NodeSet needs the Document its nodes belong to"
    );
    let kind = DocKind::of(document);
    let obj = crate::bridge::typed::wrap_magnus(NodeSet {
        document: document.into(),
        kind,
        nodes: RefCell::new(NodeVec::new()),
    })?;
    /* While `wrap` allocates the object - a GC point - `document` is held only by
     * the boxed struct, where no mark sees it. Using it afterwards keeps it on
     * the machine stack across that call, pinned by the conservative scan, so
     * compaction cannot move it out from under the stored copy. */
    core::hint::black_box(document);
    Ok(obj)
}

/// A NodeSet built here, plus a fill handle whose pushes are safe.
///
/// The handle holds the `&NodeSet` derived from the unchecked typed-data read,
/// so the "this VALUE is a NodeSet" contract is discharged once per set rather
/// than once per node. Keep [`Fill::value`] rooted (on the caller's stack) for
/// as long as the handle is used: it is the object whose data the handle points
/// into.
pub struct Fill<'a> {
    value: Value,
    set: &'a NodeSet,
}

impl Fill<'_> {
    /// The NodeSet itself, for returning to Ruby.
    pub fn value(&self) -> Value {
        self.value
    }

    /// Append one node of this set's document. `Err` for the fail-closed
    /// refusals and a failed growth.
    #[inline]
    pub fn push(&self, node: NodeWord) -> Result<(), PushError> {
        self.set.try_push(node)
    }

    /// Append every node of `nodes` (see [`push`](Self::push)).
    pub fn extend(&self, nodes: impl Iterator<Item = NodeWord>) -> Result<(), PushError> {
        self.set.try_extend(nodes)
    }

    /// Room for `n` more nodes, so the pushes that follow grow nothing.
    pub fn reserve(&self, n: usize) -> Result<(), PushError> {
        self.set.try_reserve(n)
    }
}

/// Build a NodeSet over `document` and return it with a [`Fill`] handle.
pub fn node_set_with_fill<'a>(document: Value) -> Result<(Value, Fill<'a>), Error> {
    let value = node_set_new(document)?;
    // SAFETY: built as a NodeSet by the line above, so the type is known and
    // the conversion cannot raise - which is what lets it skip the checked one
    // (a quarter of the throughput on the per-node path). `value` is returned
    // alongside, so the caller keeps it rooted for the reference's lifetime.
    let set: &NodeSet = unsafe { typed_data_unprotected(value.as_raw()) };
    Ok((value, Fill { value, set }))
}

/// A NodeSet over `document` holding `nodes`, for a caller that owns them -
/// or holds a document read they borrow. Making and growing the set can fail
/// (`NoMemoryError`), which comes back as `Err` like every allocation of
/// `raw`'s, so the caller's collection and read drop normally.
pub fn node_set_from(
    document: Value,
    nodes: impl Iterator<Item = NodeWord>,
) -> Result<Value, Error> {
    let (set, fill) = node_set_with_fill(document)?;
    fill.extend(nodes)?;
    Ok(set)
}

/// The most nodes [`node_set_after_read`] lifts out of a read onto the stack,
/// as a node's children or ancestors usually are, rather than building the
/// set while the read is held, under `protect`, which costs a `setjmp`.
const LIFT_NODES: usize = 32;

/// A NodeSet over `document` of the nodes `each` lists from `read`, a document
/// read guard - built with the read ended before anything can raise past it.
///
/// A read guard is a counted borrow: a raise that `longjmp`s past it leaves
/// the document counted as read for good, and every later edit refused. So up
/// to [`LIFT_NODES`] node words are copied to the stack (no `Drop`), the guard
/// is dropped, and the set is built after; more, and `each` runs again under
/// `protect` with the guard held, filling the set directly.
pub fn node_set_after_read<G>(
    read: G,
    document: Value,
    each: impl Fn(&G, &mut dyn FnMut(NodeWord)),
) -> Result<Value, Error> {
    let mut lifted = [NodeWord::NULL; LIFT_NODES];
    let mut len = 0usize;
    let mut overflow = false;
    each(&read, &mut |w| match lifted.get_mut(len) {
        Some(slot) => {
            *slot = w;
            len += 1;
        }
        None => overflow = true,
    });
    if overflow {
        let mut refused = None;
        let (set, fill) = node_set_with_fill(document)?;
        each(&read, &mut |w| {
            if refused.is_none() {
                if let Err(e) = fill.push(w) {
                    refused = Some(e);
                }
            }
        });
        drop(read);
        return match refused {
            Some(e) => Err(e.into()),
            None => Ok(set),
        };
    }
    drop(read);
    let (set, fill) = node_set_with_fill(document)?;
    fill.reserve(len)?;
    for &w in &lifted[..len] {
        fill.push(w)?;
    }
    Ok(set)
}

/* ------------------------------------------------------------------ */
/* the set's own operations                                           */
/* ------------------------------------------------------------------ */
/* What `glue::node_set`'s Ruby methods are built on. These only move words
 * between sets of one document (or take them from a checked node, in
 * `node_set_of_nodes`), so none of them can put a foreign word where
 * [`wrap_doc_node`] would read it; new nodes from a walk arrive through [`Fill`]. */

impl NodeSet {
    /// The number of nodes.
    pub fn count(&self) -> Result<usize, Error> {
        Ok(self.read()?.len())
    }

    /// The node at `i`, wrapped; `None` past the end.
    ///
    /// O(1): the one word is read under a short borrow that is released
    /// before the wrap, which allocates and so can run arbitrary Ruby.
    pub fn at(&self, ruby: &Ruby, i: usize) -> Result<Option<Value>, Error> {
        let node = self.read()?.as_slice().get(i).copied();
        node.map(|n| {
            // SAFETY: a node this set stored, under its own `kind`, and its
            // document is rooted by the set.
            unsafe { wrap_doc_node(self.kind, n, self.document(ruby)) }
        })
        .transpose()
    }

    /// A new set of `len` nodes from `beg`, clamped to the end; `None` when
    /// `beg` is past it.
    pub fn slice(&self, ruby: &Ruby, beg: usize, len: usize) -> Result<Option<Value>, Error> {
        let Some(room) = self.count()?.checked_sub(beg) else {
            return Ok(None);
        };
        self.take(ruby, beg, len.min(room)).map(Some)
    }

    /// A new set holding every node of this one.
    pub fn copy(&self, ruby: &Ruby) -> Result<Value, Error> {
        self.take(ruby, 0, self.count()?)
    }

    /// A new set of at most `len` nodes from `beg`.
    fn take(&self, ruby: &Ruby, beg: usize, len: usize) -> Result<Value, Error> {
        let (result, mut w) = new_result_with_room(self.document(ruby), len)?;
        let mine = self.read()?;
        let tail = mine.as_slice().get(beg..).unwrap_or_default();
        for &n in &tail[..len.min(tail.len())] {
            w.push(n)?;
        }
        drop(w);
        Ok(result)
    }

    /// `other` as a set that can be combined with this one: a NodeSet, and of
    /// the same document.
    ///
    /// A result set holds exactly one document (the GC keepalive) and one
    /// representation flag to wrap its nodes, so mixing two documents - which
    /// also means possibly mixing HTML and XML - would wrap a node under the
    /// wrong representation and fail to keep its document alive. Fail closed.
    pub fn operand<'a>(&self, ruby: &Ruby, other: Value) -> Result<&'a NodeSet, Error> {
        if !is_kind_of(other, &CLASS_NODE_SET) {
            return Err(Error::new(
                ruby.exception_type_error(),
                "expected a Makiri::NodeSet",
            ));
        }
        let o = <&NodeSet>::try_convert(other)?;
        if !crate::bridge::ruby::same_value(o.document(ruby), self.document(ruby)) {
            return Err(makiri_error(
                "cannot combine node sets from different documents",
            ));
        }
        Ok(o)
    }

    /* The set operators. Results preserve encounter order (self first) and
     * dedupe by node identity; document order is NOT imposed - that is the
     * XPath engine's job, and these mirror Nokogiri's operators.
     *
     * None of them copies an operand. The borrows are held across the pushes
     * into the result, which is sound for two reasons: the operands are only
     * read (two immutable borrows, so `a | a` is fine), and the result is a
     * fresh object, so its mutable borrow is a different cell. A push can
     * trigger a GC, and `mark` takes no borrow - see the note on the struct. An
     * earlier version snapshotted each operand into a Vec to sidestep all this
     * and measured about half the C's throughput.
     *
     * Each sizes its result by the operands first, under short borrows, and
     * only then takes the long ones, so the pushes fit and allocate nothing. */

    /// `self | other`: the union, deduped, self first.
    pub fn union(&self, ruby: &Ruby, other: &NodeSet) -> Result<Value, Error> {
        let room = self.count()?.saturating_add(other.count()?);
        let (result, mut w) = new_result_with_room(self.document(ruby), room)?;
        let (mine, theirs) = (self.read()?, other.read()?);
        let mut seen = Index::empty(mine.len() + theirs.len());
        for &n in mine.as_slice().iter().chain(theirs.as_slice()) {
            if seen.insert(n, w.as_slice()) {
                w.push(n)?;
            }
        }
        drop(w);
        Ok(result)
    }

    /// `self + other`: the concatenation, duplicates kept.
    pub fn concat(&self, ruby: &Ruby, other: &NodeSet) -> Result<Value, Error> {
        let room = self.count()?.saturating_add(other.count()?);
        let (result, mut w) = new_result_with_room(self.document(ruby), room)?;
        let (mine, theirs) = (self.read()?, other.read()?);
        for &n in mine.as_slice().iter().chain(theirs.as_slice()) {
            w.push(n)?;
        }
        drop(w);
        Ok(result)
    }

    /// `&` ([`Membership::In`]) and `-` ([`Membership::NotIn`]): each node of
    /// self with that membership in `other`, deduped, in self's order.
    pub fn filter(&self, ruby: &Ruby, other: &NodeSet, keep: Membership) -> Result<Value, Error> {
        let (result, mut w) = new_result_with_room(self.document(ruby), self.count()?)?;
        let (mine, theirs) = (self.read()?, other.read()?);
        let theirs_index = Index::build(theirs.as_slice());
        let mut seen = Index::empty(mine.len());
        for &n in mine.as_slice() {
            if theirs_index.contains(n, theirs.as_slice()) != (keep == Membership::In) {
                continue;
            }
            if seen.insert(n, w.as_slice()) {
                w.push(n)?;
            }
        }
        drop(w);
        Ok(result)
    }
}

/// A new set over `document` holding `nodes`, each of which must be a Makiri
/// node of that document.
///
/// The check is the set's invariant, not an argument nicety: a node from
/// another document or representation would be re-wrapped under the wrong
/// document and kind - exactly the HTML/XML confusion the set's opaque storage
/// relies on the document to prevent.
pub fn node_set_of_nodes(
    ruby: &Ruby,
    document: Value,
    nodes: impl Iterator<Item = Value>,
) -> Result<Value, Error> {
    let (set, s) = new_result(document)?;
    let mut w = s.write()?;
    for item in nodes {
        if !is_kind_of(item, &CLASS_NODE)
            || !crate::bridge::ruby::same_value(keepalive_document(item)?, document)
        {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "every node must be a Makiri node belonging to the given document",
            ));
        }
        w.push(node_raw(item)?)?;
    }
    drop(w);
    Ok(set)
}

/// An empty NodeSet over `document`, plus the handle to fill it through.
///
/// The reference's lifetime is unconstrained, as magnus's own `try_convert` for
/// a wrapped type gives: the data lives as long as the Ruby object, which the
/// returned `Value` keeps rooted on the caller's stack.
fn new_result<'a>(document: Value) -> Result<(Value, &'a NodeSet), Error> {
    /* The one unchecked borrow of a fresh set is `node_set_with_fill`'s. */
    let (set, fill) = node_set_with_fill(document)?;
    Ok((set, fill.set))
}

/// Which nodes of self [`NodeSet::filter`] keeps: those in the other set
/// (`&`), or those not in it (`-`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    In,
    NotIn,
}

/// [`new_result`] with room for `room` nodes, and its write borrow.
///
/// The set's allocations happen here, before the caller holds anything but
/// this borrow of a set nothing else can reach yet.
fn new_result_with_room<'a>(
    document: Value,
    room: usize,
) -> Result<(Value, std::cell::RefMut<'a, NodeVec>), Error> {
    let (result, r) = new_result(document)?;
    let mut w = r.write()?;
    w.reserve(room)?;
    Ok((result, w))
}

/* ---- membership, for the operators ---- */

type PtrBuild = core::hash::BuildHasherDefault<crate::ptr_table::MixHasher>;
type PtrSet = HashSet<NodeWord, PtrBuild>;

/// Membership over a node array: hashed above [`HASH_MIN`], scanned below it.
///
/// `try_reserve` rather than `reserve`, so an allocation failure degrades to
/// the linear scan - the C's "cap == 0 means not built" fallback - instead of
/// aborting the process.
enum Index {
    Hashed(PtrSet),
    Linear,
}

impl Index {
    fn build(nodes: &[NodeWord]) -> Index {
        if nodes.len() <= HASH_MIN {
            return Index::Linear;
        }
        let mut set = PtrSet::default();
        if set.falloc_reserve(nodes.len()).is_err() {
            return Index::Linear;
        }
        set.extend(nodes.iter().copied());
        Index::Hashed(set)
    }

    /// Sized for what is about to be inserted, holding nothing yet.
    fn empty(expected: usize) -> Index {
        if expected <= HASH_MIN {
            return Index::Linear;
        }
        let mut set = PtrSet::default();
        if set.falloc_reserve(expected).is_err() {
            return Index::Linear;
        }
        Index::Hashed(set)
    }

    fn contains(&self, n: NodeWord, fallback: &[NodeWord]) -> bool {
        match self {
            Index::Hashed(s) => s.contains(&n),
            Index::Linear => fallback.contains(&n),
        }
    }

    /// True when `n` had not been seen. `already` is what the linear fallback
    /// scans - the result built so far.
    fn insert(&mut self, n: NodeWord, already: &[NodeWord]) -> bool {
        match self {
            Index::Hashed(s) => s.insert(n),
            Index::Linear => !already.contains(&n),
        }
    }
}
