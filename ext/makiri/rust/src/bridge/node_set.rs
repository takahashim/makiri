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
//! It owns its `rb_data_type_t`, its GC `mark`/`size`/free, and a node buffer
//! whose bytes it reports to Ruby's GC, so its layout is private and it belongs
//! with the other raw-Ruby-ABI seams rather than in the glue.
//!
//! The buffer is a `Vec` grown through `falloc`, not Ruby's allocator: the
//! report is exactly the bytes it holds, as a Document's arena report is.
//! (It was `ruby_xrealloc2`'s, behind a proven `RawVec`; the `Vec` measured
//! 5-10% faster on large sets, and Ruby counted the reallocations ~20% over
//! the bytes held, so it collected more often than the memory called for.) The glue builds a set through
//! [`FreshSet`], [`node_set_from`], [`node_set_from_read`] or
//! [`node_set_of_nodes`], all over the Document's opaque `VALUE`; the
//! [`Fill`] handle carries the "this is a NodeSet" invariant so its pushes
//! are safe.
//!
//! Mutation goes through a `RefCell`, and every borrow failure becomes a Ruby
//! error rather than a panic: a panic would reach Ruby as
//! `Makiri::InternalError`, a broken invariant, which is the wrong answer for
//! an aliasing mistake a Ruby caller can provoke - this codebase fails closed
//! by raising an ordinary error.

#![forbid(unsafe_code)]

use falloc::{Reserve, SetInsert, VecGrow};
use std::cell::RefCell;
use std::collections::HashSet;

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::bridge::ruby::makiri_error;

use crate::bridge::node_wrap::wrap_doc_node;
use crate::bridge::typed::{Birth, Payload, Typed, TypedType};
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
/// the set itself, and past that a heap `Vec` that holds them all.
struct NodeVec {
    inline: [NodeWord; INLINE_NODES],
    /// The nodes in `inline`; 0 once the set has spilled to `heap`.
    inline_len: usize,
    /// Every node once the set has spilled (its capacity is then non-zero);
    /// empty, holding no allocation, before. On the system allocator through
    /// `falloc`, so `rake oom` fails each growth; its bytes are reported to
    /// Ruby's GC as a Document's arena is (`reported`).
    heap: Vec<NodeWord>,
    /// The heap's bytes as last reported to the GC, given back when the set
    /// is freed.
    reported: crate::bridge::typed::External,
}

/// Why a push was refused.
///
/// Deliberately small rather than a `crate::bridge::rb::Error`: a push runs once per node
/// of every result, and returning the large error type from each successful
/// push measured about 2ns a node. Callers convert through `From` on the failure path only.
#[derive(Clone, Copy)]
pub enum PushError {
    SizeLimit,
    /// Ruby's allocator raised `NoMemoryError`, which this answers again.
    NoMemory,
    /// The set is borrowed elsewhere - a push from inside its own iteration.
    Busy,
}

impl From<PushError> for Error {
    fn from(e: PushError) -> Error {
        match e {
            PushError::SizeLimit => makiri_error(format_args!(
                "node set size limit exceeded ({NODE_SET_MAX} nodes)"
            )),
            PushError::NoMemory => Error::new(
                crate::bridge::rb::Builtin::NoMemoryError,
                "failed to allocate memory",
            ),
            PushError::Busy => makiri_error("node set is already in use"),
        }
    }
}

impl NodeVec {
    const fn new() -> Self {
        NodeVec {
            inline: [NodeWord::NULL; INLINE_NODES],
            inline_len: 0,
            heap: Vec::new(),
            reported: crate::bridge::typed::External::new(),
        }
    }

    /// Tell the GC how far the heap's bytes moved since the last report: a
    /// growth is memory Ruby did not allocate and would not otherwise see.
    /// Called after each growth, which is under the GVL (a fill runs in a
    /// method).
    fn account(&mut self, ruby: &Ruby) {
        let bytes = self
            .heap
            .capacity()
            .saturating_mul(core::mem::size_of::<NodeWord>());
        self.reported.report(ruby, bytes);
    }

    /// Room for `want` nodes in all on the heap.
    fn grow_to(&mut self, ruby: &crate::bridge::rb::Ruby, want: usize) -> Result<(), PushError> {
        let more = want.saturating_sub(self.heap.len());
        self.heap
            .falloc_reserve(more)
            .map_err(|()| PushError::NoMemory)?;
        self.account(ruby);
        Ok(())
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
    fn spill(&mut self, ruby: &crate::bridge::rb::Ruby, n: usize) -> Result<(), PushError> {
        self.grow_to(ruby, n.max(self.inline_len))?;
        for &w in &self.inline[..self.inline_len] {
            /* Room was reserved above, so this grows nothing. */
            self.heap.falloc_push(w).map_err(|()| PushError::NoMemory)?;
        }
        self.inline_len = 0;
        Ok(())
    }

    /// Append to the heap, growing it (and reporting the growth) when full.
    #[inline]
    fn heap_push(&mut self, ruby: &crate::bridge::rb::Ruby, w: NodeWord) -> Result<(), PushError> {
        let full = self.heap.len() == self.heap.capacity();
        self.heap.falloc_push(w).map_err(|()| PushError::NoMemory)?;
        if full {
            self.account(ruby);
        }
        Ok(())
    }

    /// Make room for `n` nodes (at most [`NODE_SET_MAX`]) in one allocation,
    /// so the pushes that follow cannot reallocate. Room that fits inline
    /// takes none.
    fn reserve(&mut self, ruby: &crate::bridge::rb::Ruby, n: usize) -> Result<(), PushError> {
        let n = n.min(NODE_SET_MAX);
        if self.spilled() {
            self.grow_to(ruby, n)
        } else if n > INLINE_NODES {
            self.spill(ruby, n)
        } else {
            Ok(())
        }
    }

    /// Append every node of `nodes`, as [`push`](Self::push) each: room for
    /// the iterator's lower bound is taken first, and a set on the heap is
    /// then filled in one loop, the size cap checked once for the whole run.
    fn extend(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        nodes: impl Iterator<Item = NodeWord>,
    ) -> Result<(), PushError> {
        let mut nodes = nodes.peekable();
        self.reserve(ruby, self.len().saturating_add(nodes.size_hint().0))?;
        while nodes.peek().is_some() {
            if self.spilled() {
                let room = NODE_SET_MAX.saturating_sub(self.heap.len());
                for w in nodes.by_ref().take(room) {
                    self.heap_push(ruby, w)?;
                }
                if nodes.peek().is_some() {
                    return Err(PushError::SizeLimit);
                }
                return Ok(());
            }
            if let Some(w) = nodes.next() {
                self.push(ruby, w)?;
            }
        }
        Ok(())
    }

    /// Append one node: `Err` for the size cap or a failed allocation.
    #[inline]
    fn push(&mut self, ruby: &crate::bridge::rb::Ruby, node: NodeWord) -> Result<(), PushError> {
        if self.spilled() {
            if self.heap.len() >= NODE_SET_MAX {
                return Err(PushError::SizeLimit);
            }
            return self.heap_push(ruby, node);
        }
        if let Some(slot) = self.inline.get_mut(self.inline_len) {
            *slot = node;
            self.inline_len += 1;
            return Ok(());
        }
        /* Full inline: spill, with room to grow (NODE_SET_MAX is far above
         * INLINE_NODES, so the cap is not reached here). */
        self.spill(ruby, INLINE_NODES * 4)?;
        self.heap_push(ruby, node)
    }
}

/// Only the node array is mutable, and only it is behind the cell.
///
/// A GC can happen *while a push holds the mutable borrow* - a growth reports
/// its bytes to the GC, which may collect - and the size and free callbacks
/// read the set then: they take no `&mut`, and read a busy cell as empty.
/// The Document is not a field at all but the object's slot 0, which `nupur`
/// marks without a borrow. Fields that never change after construction belong
/// outside the cell.
///
/// A [`TypedType`] like every other wrapped struct of Makiri's, but not an
/// embedded one: a fill holds `&NodeSet` across the set's own growth, which
/// is an allocation, and compaction would move embedded data out from under
/// it.
pub struct NodeSet {
    /* The owning Document is not here: it is the object's slot 0
     * ([`NodeSetType`]), read through the set's owner ([`NodeSetSelf`],
     * [`FreshSet`]). */
    /// Decided once: whether the stored words are XML arena handles (wrapped as
    /// `Makiri::XML::*`) or HTML arena nodes.
    kind: DocKind,
    nodes: RefCell<NodeVec>,
}

impl Payload for NodeSet {
    fn memsize(&self) -> usize {
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

    /// What the heap's growth reported, which the free callback gives back,
    /// as a Document's arena report is. Read without a borrow: the set is
    /// being freed, so nothing holds one.
    fn reported_external(&self) -> usize {
        self.nodes.try_borrow().map_or(0, |n| n.reported.reported())
    }
}

/// A NodeSet's type: [`NodeSet`], and in slot 0 the Document whose nodes it
/// holds - marked by `nupur` on every GC, a push holding the cell or not.
pub type NodeSetType = TypedType<NodeSet, 1>;

/// `Makiri::NodeSet`'s data type.
pub static NODE_SET_TYPE: NodeSetType = TypedType::base(c"Makiri::NodeSet");

/// A `NodeSet` method's receiver, or an operand checked to be a set: the
/// object, read as its [`NodeSet`] for the call through the [`Typed`] handle
/// that holds it.
///
/// Its type is checked once, when it is made: `NodeSet#each` reads the
/// nodes, the kind and the Document once per element.
pub struct NodeSetSelf(Typed<NodeSet, 1>);

impl crate::bridge::rb::TryConvert for NodeSetSelf {
    fn try_convert(_: &Ruby, value: Value) -> Result<Self, Error> {
        NODE_SET_TYPE.check(value).map(NodeSetSelf)
    }
}

impl core::ops::Deref for NodeSetSelf {
    type Target = NodeSet;
    fn deref(&self) -> &NodeSet {
        self.0.data()
    }
}

impl NodeSetSelf {
    /// The set itself.
    pub fn value(&self) -> Value {
        self.0.value()
    }

    /// The Document whose nodes the set holds.
    fn document(&self) -> Value {
        self.0.root::<0>()
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

    /// Append a node to a set this module built or checked; see [`Fill`]. `Err`
    /// only for the fail-closed refusals (size cap, busy) and a failed growth.
    #[inline]
    fn try_push(&self, ruby: &crate::bridge::rb::Ruby, node: NodeWord) -> Result<(), PushError> {
        let Ok(mut nodes) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        nodes.push(ruby, node)
    }

    /// [`NodeVec::extend`] on a set this module built or checked.
    fn try_extend(
        &self,
        ruby: &crate::bridge::rb::Ruby,
        nodes: impl Iterator<Item = NodeWord>,
    ) -> Result<(), PushError> {
        let Ok(mut set) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        set.extend(ruby, nodes)
    }

    /// Room for `n` more nodes in one allocation.
    fn try_reserve(&self, ruby: &crate::bridge::rb::Ruby, n: usize) -> Result<(), PushError> {
        let Ok(mut nodes) = self.nodes.try_borrow_mut() else {
            return Err(PushError::Busy);
        };
        let want = nodes.len().saturating_add(n);
        nodes.reserve(ruby, want)
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
fn node_set_new(ruby: &Ruby, document: Value) -> Result<Typed<NodeSet, 1>, Error> {
    assert!(
        is_kind_of(ruby, document, &CLASS_DOCUMENT),
        "a NodeSet needs the Document its nodes belong to"
    );
    let kind = DocKind::of(document);
    NODE_SET_TYPE.wrap(
        ruby,
        Birth {
            klass: CLASS_NODE_SET.defined(ruby)?,
            data: NodeSet {
                kind,
                nodes: RefCell::new(NodeVec::new()),
            },
            roots: [document],
        },
    )
}

/// A NodeSet just built here, held by its maker, and the owner every borrow
/// of its data is tied to: a [`Fill`]
/// ([`fill`](Self::fill)) or a write ([`with_room`](Self::with_room)) cannot
/// outlive it.
///
/// Nothing but this frame refers to the set until it is returned: it is the
/// [`Typed`] handle the wrap made, so the [`Fill`]'s borrow of its data keeps
/// the object where the GC sees it.
pub struct FreshSet(Typed<NodeSet, 1>);

impl FreshSet {
    /// An empty NodeSet over `document`.
    pub fn new(ruby: &Ruby, document: Value) -> Result<FreshSet, Error> {
        node_set_new(ruby, document).map(FreshSet)
    }

    /// The NodeSet itself, for returning to Ruby.
    pub fn value(&self) -> Value {
        self.0.value()
    }

    fn set(&self) -> &NodeSet {
        self.0.data()
    }

    /// A handle to append through, borrowing the set.
    pub fn fill<'a>(&'a self, ruby: &'a Ruby) -> Fill<'a> {
        Fill {
            set: self.set(),
            ruby,
        }
    }

    /// The set's write borrow, with room for `room` nodes: its allocation
    /// happens here, before the caller holds anything but this borrow of a
    /// set nothing else can reach yet.
    fn with_room(&self, ruby: &Ruby, room: usize) -> Result<std::cell::RefMut<'_, NodeVec>, Error> {
        let mut w = self.set().write()?;
        w.reserve(ruby, room)?;
        Ok(w)
    }

    /// A NodeSet over `document`, filled by `fill` through its write borrow
    /// ([`with_room`](Self::with_room), room for `room` nodes taken first):
    /// what every operation that makes a set from sets it already has is.
    fn build(
        ruby: &Ruby,
        document: Value,
        room: usize,
        fill: impl FnOnce(&mut NodeVec) -> Result<(), Error>,
    ) -> Result<Value, Error> {
        let result = FreshSet::new(ruby, document)?;
        fill(&mut *result.with_room(ruby, room)?)?;
        Ok(result.value())
    }
}

/// The pushes into a [`FreshSet`]: one borrow of the set, so the "this
/// VALUE is a NodeSet" check is made once per set rather than once per node.
pub struct Fill<'a> {
    set: &'a NodeSet,
    ruby: &'a Ruby,
}

impl Fill<'_> {
    /// Append one node of this set's document. `Err` for the fail-closed
    /// refusals and a failed growth.
    #[inline]
    pub fn push(&self, node: NodeWord) -> Result<(), PushError> {
        self.set.try_push(self.ruby, node)
    }

    /// Append every node of `nodes` (see [`push`](Self::push)).
    pub fn extend(&self, nodes: impl Iterator<Item = NodeWord>) -> Result<(), PushError> {
        self.set.try_extend(self.ruby, nodes)
    }

    /// Room for `n` more nodes, so the pushes that follow grow nothing.
    pub fn reserve(&self, n: usize) -> Result<(), PushError> {
        self.set.try_reserve(self.ruby, n)
    }
}

/// A NodeSet over `document` holding `nodes`, for a caller that owns them -
/// or holds a document read they borrow. Making and growing the set can fail
/// (`NoMemoryError`), which comes back as `Err` like every allocation of
/// `raw`'s, so the caller's collection and read drop normally.
pub fn node_set_from(
    ruby: &Ruby,
    document: Value,
    nodes: impl Iterator<Item = NodeWord>,
) -> Result<Value, Error> {
    let set = FreshSet::new(ruby, document)?;
    set.fill(ruby).extend(nodes)?;
    Ok(set.value())
}

/// A NodeSet over `document` of the nodes `each` lists from `read`, a document
/// read guard, built while the read is held. A failed allocation is `Err`
/// (`nupur::raw`), so the guard drops either way.
pub fn node_set_from_read<G>(
    ruby: &Ruby,
    read: G,
    document: Value,
    each: impl Fn(&G, &mut dyn FnMut(NodeWord)),
) -> Result<Value, Error> {
    let mut refused = None;
    let set = FreshSet::new(ruby, document)?;
    let fill = set.fill(ruby);
    each(&read, &mut |w| {
        if refused.is_none() {
            if let Err(e) = fill.push(w) {
                refused = Some(e);
            }
        }
    });
    drop(read);
    match refused {
        Some(e) => Err(e.into()),
        None => Ok(set.value()),
    }
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
}

/* The operations that read the set's Document take its owner, which holds it
 * in a slot. */
impl NodeSetSelf {
    /// The node at `i`, wrapped; `None` past the end.
    ///
    /// O(1): the one word is read under a short borrow that is released
    /// before the wrap, which needs nothing of it.
    pub fn at(&self, ruby: &Ruby, i: usize) -> Result<Option<Value>, Error> {
        let node = self.read()?.as_slice().get(i).copied();
        node.map(|n| wrap_doc_node(ruby, self.kind, n, self.document()))
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
        FreshSet::build(ruby, self.document(), len, |w| {
            let mine = self.read()?;
            let tail = mine.as_slice().get(beg..).unwrap_or_default();
            for &n in &tail[..len.min(tail.len())] {
                w.push(ruby, n)?;
            }
            Ok(())
        })
    }

    /// `other` as a set that can be combined with this one: a NodeSet, and of
    /// the same document.
    ///
    /// A result set holds exactly one document (the GC keepalive) and one
    /// representation flag to wrap its nodes, so mixing two documents - which
    /// also means possibly mixing HTML and XML - would wrap a node under the
    /// wrong representation and fail to keep its document alive. Fail closed.
    pub fn operand(&self, ruby: &Ruby, other: Value) -> Result<NodeSetSelf, Error> {
        if !is_kind_of(ruby, other, &CLASS_NODE_SET) {
            return Err(Error::new(
                crate::bridge::rb::Builtin::TypeError,
                "expected a Makiri::NodeSet",
            ));
        }
        let o = NodeSetSelf::try_convert(ruby, other)?;
        if !crate::bridge::ruby::same_value(o.document(), self.document()) {
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
        FreshSet::build(ruby, self.document(), room, |w| {
            let (mine, theirs) = (self.read()?, other.read()?);
            let mut seen = Index::empty(mine.len() + theirs.len());
            for &n in mine.as_slice().iter().chain(theirs.as_slice()) {
                if seen.insert(n, w.as_slice()) {
                    w.push(ruby, n)?;
                }
            }
            Ok(())
        })
    }

    /// `self + other`: the concatenation, duplicates kept.
    pub fn concat(&self, ruby: &Ruby, other: &NodeSet) -> Result<Value, Error> {
        let room = self.count()?.saturating_add(other.count()?);
        FreshSet::build(ruby, self.document(), room, |w| {
            let (mine, theirs) = (self.read()?, other.read()?);
            for &n in mine.as_slice().iter().chain(theirs.as_slice()) {
                w.push(ruby, n)?;
            }
            Ok(())
        })
    }

    /// `&` ([`Membership::In`]) and `-` ([`Membership::NotIn`]): each node of
    /// self with that membership in `other`, deduped, in self's order.
    pub fn filter(&self, ruby: &Ruby, other: &NodeSet, keep: Membership) -> Result<Value, Error> {
        FreshSet::build(ruby, self.document(), self.count()?, |w| {
            let (mine, theirs) = (self.read()?, other.read()?);
            let theirs_index = Index::build(theirs.as_slice());
            let mut seen = Index::empty(mine.len());
            for &n in mine.as_slice() {
                if theirs_index.contains(n, theirs.as_slice()) != (keep == Membership::In) {
                    continue;
                }
                if seen.insert(n, w.as_slice()) {
                    w.push(ruby, n)?;
                }
            }
            Ok(())
        })
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
    FreshSet::build(ruby, document, 0, |w| {
        for item in nodes {
            if !is_kind_of(ruby, item, &CLASS_NODE)
                || !crate::bridge::ruby::same_value(keepalive_document(ruby, item)?, document)
            {
                return Err(Error::new(
                    crate::bridge::rb::Builtin::ArgumentError,
                    "every node must be a Makiri node belonging to the given document",
                ));
            }
            w.push(ruby, node_raw(ruby, item)?)?;
        }
        Ok(())
    })
}

/// Which nodes of self [`NodeSet::filter`] keeps: those in the other set
/// (`&`), or those not in it (`-`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    In,
    NotIn,
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
        for &n in nodes {
            if set.falloc_insert(n).is_err() {
                return Index::Linear;
            }
        }
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
            /* Sized for every node it will see (`empty`), so this does not
             * grow; should it fail, the scan takes over for good, since
             * `already` is the whole answer so far. */
            Index::Hashed(s) => s.falloc_insert(n).unwrap_or_else(|()| {
                *self = Index::Linear;
                !already.contains(&n)
            }),
            Index::Linear => !already.contains(&n),
        }
    }
}
