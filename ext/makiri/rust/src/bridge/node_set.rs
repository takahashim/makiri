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

use crate::bridge::alloc::RubyVec;
use crate::falloc::Reserve;
use std::cell::RefCell;
use std::collections::HashSet;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::value::{Opaque, ReprValue};
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

/// The set's node array: a [`RubyVec`] under the set's size cap.
struct NodeVec(RubyVec<NodeWord>);

/// Why a push was refused.
///
/// Deliberately small rather than a `magnus::Error`: a push runs once per node
/// of every result, and returning the large error type from each successful
/// push measured about 2ns a node. Callers convert through `From` on the failure path only.
#[derive(Clone, Copy)]
pub enum PushError {
    SizeLimit,
    CapacityOverflow,
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
            PushError::Busy => "node set is already in use".to_string(),
        }
    }
}

impl From<PushError> for Error {
    fn from(e: PushError) -> Error {
        makiri_error(e.message())
    }
}

impl NodeVec {
    const fn new() -> Self {
        NodeVec(RubyVec::new())
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn capacity(&self) -> usize {
        self.0.capacity()
    }

    fn as_slice(&self) -> &[NodeWord] {
        self.0.as_slice()
    }

    /// Make room for `n` nodes (at most [`NODE_SET_MAX`]) in one allocation,
    /// so the pushes that follow cannot reallocate - and so cannot raise.
    fn reserve(&mut self, n: usize) {
        self.0.reserve(n.min(NODE_SET_MAX));
    }

    /// Append one node. `Err` only for the size cap or a capacity overflow -
    /// allocation failure raises inside Ruby.
    fn push(&mut self, node: NodeWord) -> Result<(), PushError> {
        if self.0.len() >= NODE_SET_MAX {
            return Err(PushError::SizeLimit);
        }
        self.0.push(node).map_err(|_| PushError::CapacityOverflow)
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
#[derive(TypedData)]
#[magnus(class = "Makiri::NodeSet", mark, size, free_immediately)]
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
}

/* ------------------------------------------------------------------ */
/* the constructors the glue calls                                    */
/* ------------------------------------------------------------------ */

/// An empty NodeSet over `document`, whose nodes it will hold.
///
/// The Document decides how a stored word is read back - an arena token for
/// XML, an HTML arena node for HTML - so it is checked here rather than taken on
/// trust: one class test per set, never per node.
pub fn node_set_new(document: Value) -> Value {
    assert!(
        is_kind_of(document, &CLASS_DOCUMENT),
        "a NodeSet needs the Document its nodes belong to"
    );
    let ruby = Ruby::get_with(document);
    let kind = DocKind::of(document);
    let obj = ruby
        .wrap(NodeSet {
            document: document.into(),
            kind,
            nodes: RefCell::new(NodeVec::new()),
        })
        .as_value();
    /* While `wrap` allocates the object - a GC point - `document` is held only by
     * the boxed struct, where no mark sees it. Using it afterwards keeps it on
     * the machine stack across that call, pinned by the conservative scan, so
     * compaction cannot move it out from under the stored copy. */
    core::hint::black_box(document);
    obj
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
    /// refusals only; growth raises inside Ruby.
    #[inline]
    pub fn push(&self, node: NodeWord) -> Result<(), PushError> {
        self.set.try_push(node)
    }
}

/// Build a NodeSet over `document` and return it with a [`Fill`] handle.
pub fn node_set_with_fill<'a>(document: Value) -> (Value, Fill<'a>) {
    let value = node_set_new(document);
    // SAFETY: built as a NodeSet by the line above, so the type is known and
    // the conversion cannot raise - which is what lets it skip the checked one
    // (a quarter of the throughput on the per-node path). `value` is returned
    // alongside, so the caller keeps it rooted for the reference's lifetime.
    let set: &NodeSet = unsafe { typed_data_unprotected(value.as_raw()) };
    (value, Fill { value, set })
}

/// A NodeSet over `document` holding `nodes`, for a caller that owns them -
/// or holds a document read they borrow.
///
/// Built under `protect`, the set's allocation included: making and growing
/// the set can raise `NoMemoryError` from Ruby's allocator, and a longjmp
/// would skip the drop of whatever collection the caller is iterating, and of
/// a read guard the nodes borrow. Under `protect` it comes back as `Err`
/// instead, those drop normally, and magnus raises afterwards - one setjmp
/// per set, not per node.
pub fn node_set_from(
    document: Value,
    nodes: impl Iterator<Item = NodeWord>,
) -> Result<Value, Error> {
    let mut refused = None;
    /* Borrowed into the protected closure, not moved: a raise unwinds the
     * closure's frames only, and the collection the iterator owns (a
     * matcher's result) is still this frame's to drop. */
    let mut nodes = nodes;
    let set = crate::bridge::ruby::protect_value(|| {
        let (set, fill) = node_set_with_fill(document);
        for n in nodes.by_ref() {
            if let Err(e) = fill.push(n) {
                refused = Some(e);
                break;
            }
        }
        set.as_raw()
    })?;
    match refused {
        Some(e) => Err(e.into()),
        None => Ok(set),
    }
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
        let set = crate::bridge::ruby::protect_value(|| {
            let (set, fill) = node_set_with_fill(document);
            each(&read, &mut |w| {
                if refused.is_none() {
                    if let Err(e) = fill.push(w) {
                        refused = Some(e);
                    }
                }
            });
            set.as_raw()
        })?;
        drop(read);
        return match refused {
            Some(e) => Err(e.into()),
            None => Ok(set),
        };
    }
    drop(read);
    let (set, fill) = node_set_with_fill(document);
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
     * What they must not do is hold those borrows across a push that grows the
     * result: growing is `ruby_xrealloc2`, whose `NoMemoryError` longjmps past
     * every drop, leaving an operand's borrow counted for good (the set refuses
     * every later write) and leaking the membership index. So each sizes its
     * result by the operands first, under short borrows, and only then takes
     * the long ones; the pushes fit and cannot raise. */

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
    let (set, s) = new_result(document);
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
fn new_result<'a>(document: Value) -> (Value, &'a NodeSet) {
    /* The one unchecked borrow of a fresh set is `node_set_with_fill`'s. */
    let (set, fill) = node_set_with_fill(document);
    (set, fill.set)
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
/// The one allocation that can raise happens here, before the caller holds
/// anything but this borrow of a set nothing else can reach yet.
fn new_result_with_room<'a>(
    document: Value,
    room: usize,
) -> Result<(Value, std::cell::RefMut<'a, NodeVec>), Error> {
    let (result, r) = new_result(document);
    let mut w = r.write()?;
    w.reserve(room);
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
