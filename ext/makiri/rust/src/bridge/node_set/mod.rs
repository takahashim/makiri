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
//! the bytes held, so it collected more often than the memory called for.)
//! Its storage is [`storage`]; the membership the set operators build is
//! [`index`].
//!
//! A set is built here and nowhere else: the glue makes one through
//! [`node_set_from`], [`node_set_from_read`] or [`node_set_of_nodes`], all
//! over the Document's opaque `VALUE`, and the set's own operations make the
//! rest. Each is filled in place - the object first, so what its growth
//! reports to the GC is given back by its free callback however the fill
//! ends - and is never changed once returned.
//!
//! Filling in place is what the contents' `RefCell` is for. No Ruby code
//! runs while a set is borrowed, and none can reach a set being filled, so
//! a borrow that conflicts is a broken invariant, answered as
//! `Makiri::InternalError` rather than a panic.

#![forbid(unsafe_code)]

use std::cell::RefCell;

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::bridge::ruby::makiri_error;

use crate::bridge::node_wrap::wrap_doc_node;
use crate::bridge::typed::{Birth, Payload, Typed, TypedType};
use crate::bridge::wrapper::{node_word_in, DocKind, NodeWord, NotInDocument};
use crate::init::{CLASS_DOCUMENT, CLASS_NODE, CLASS_NODE_SET};

/// Is `v` an instance of `klass`?
use crate::bridge::ruby::is_kind_of;

mod index;
mod storage;

use index::Index;
use storage::NodeVec;
pub use storage::PushError;

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
    /// Borrow the contents. A conflict is a broken invariant - no Ruby can
    /// reach a set while it is borrowed (module docs) - answered as
    /// `InternalError` rather than a panic.
    fn read(&self) -> Result<std::cell::Ref<'_, NodeVec>, Error> {
        self.nodes.try_borrow().map_err(|_| busy())
    }

    fn write(&self) -> Result<std::cell::RefMut<'_, NodeVec>, Error> {
        self.nodes.try_borrow_mut().map_err(|_| busy())
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
}

/// A set borrowed where nothing can borrow it: a broken invariant.
fn busy() -> Error {
    crate::bridge::ruby::internal_error("a node set borrowed twice")
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
struct FreshSet(Typed<NodeSet, 1>);

impl FreshSet {
    /// An empty NodeSet over `document`.
    fn new(ruby: &Ruby, document: Value) -> Result<FreshSet, Error> {
        node_set_new(ruby, document).map(FreshSet)
    }

    /// The NodeSet itself, for returning to Ruby.
    fn value(&self) -> Value {
        self.0.value()
    }

    fn set(&self) -> &NodeSet {
        self.0.data()
    }

    /// A handle to append through, borrowing the set.
    fn fill<'a>(&'a self, ruby: &'a Ruby) -> Fill<'a> {
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
struct Fill<'a> {
    set: &'a NodeSet,
    ruby: &'a Ruby,
}

impl Fill<'_> {
    /// Append one node of this set's document. `Err` for the fail-closed
    /// refusals and a failed growth.
    #[inline]
    fn push(&self, node: NodeWord) -> Result<(), PushError> {
        self.set.try_push(self.ruby, node)
    }

    /// Append every node of `nodes` (see [`push`](Self::push)).
    fn extend(&self, nodes: impl Iterator<Item = NodeWord>) -> Result<(), PushError> {
        self.set.try_extend(self.ruby, nodes)
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
        let refused = || {
            Error::new(
                crate::bridge::rb::Builtin::ArgumentError,
                "every node must be a Makiri node belonging to the given document",
            )
        };
        for item in nodes {
            if !is_kind_of(ruby, item, &CLASS_NODE) {
                return Err(refused());
            }
            let word = node_word_in(ruby, item, document).map_err(|e| match e {
                NotInDocument::Unusable(e) => e,
                NotInDocument::Foreign => refused(),
            })?;
            w.push(ruby, word)?;
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
