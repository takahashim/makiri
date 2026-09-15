//! `Makiri::NodeSet` (glue/ruby_node_set.c).
//!
//! A NodeSet is an array of node pointers plus a keepalive reference to the
//! owning Document. The nodes are owned by the document's arena, so marking the
//! document keeps every one of them alive - that is the whole GC contract.
//!
//! The stored pointers are representation-opaque. The set never dereferences
//! one; it compares them for identity and, when vending a node, casts to the
//! representation named by `doc_is_xml`. That keeps an XML set from ever reading
//! its `mkr_xml_node_t*` as an `lxb_dom_node_t*`. The kind is decided once at
//! construction rather than probed per node, which would regress the hot
//! traversal path.
//!
//! # This is the first Rust-owned wrapper type
//!
//! `glue::node` exports its `rb_data_type_t` because C still wraps and unwraps
//! nodes. Nothing outside this file touches the NodeSet's, so it uses magnus's
//! own [`TypedData`] instead, and the struct layout becomes private - the C API
//! other files call (`node_set_new` / `node_set_push`) is just two
//! functions over an opaque `VALUE`. That is the shape the rest of the wrapper
//! types should reach.
//!
//! Mutation goes through a `RefCell`, and every borrow failure becomes a Ruby
//! error rather than a panic: `panic = "abort"` would turn an aliasing mistake
//! into a dead process, and this codebase fails closed by raising.

use crate::falloc::Reserve;
use core::ffi::{c_long, c_void};
use std::cell::RefCell;
use std::collections::HashSet;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::value::{Opaque, ReprValue};
use magnus::{
    gc::Marker, method, prelude::*, DataTypeFunctions, Error, RArray, RClass, Ruby, TypedData,
    Value,
};
use rb_sys::VALUE;

use super::abi::{
    error_class, keepalive_document, node_raw, typed_data_unprotected, wrap_html_node,
    wrap_xml_node, LxbNode, CLASS_DOCUMENT, CLASS_NODE, CLASS_NODE_SET, CLASS_XML_DOCUMENT,
};

/// The per-set node cap, shared with the CSS and XPath glue: every
/// node-collecting path fails closed at the same bound instead of growing
/// without limit.
const NODE_SET_MAX: usize = 10 * 1000 * 1000;

/// Below this operand size a linear scan beats building a hash set.
const HASH_MIN: usize = 64;

/* ------------------------------------------------------------------ */
/* storage                                                            */
/* ------------------------------------------------------------------ */

/// A growable array of node pointers held in Ruby's allocator.
///
/// Ruby's rather than Rust's on purpose: `ruby_xrealloc2` keeps the buffer
/// GC-accounted, so a large set contributes to the pressure that decides when a
/// GC runs. It also raises `NoMemoryError` instead of returning NULL, so there
/// is no allocation-failure branch to get wrong.
///
/// This is the only unsafe block in the file; everything above it sees a slice.
struct NodeVec {
    ptr: *mut *mut c_void,
    len: usize,
    cap: usize,
}

/// Why a push was refused.
///
/// Deliberately small rather than a `magnus::Error`: a push runs once per node
/// of every result, and returning the large error type from each successful
/// push measured about 2ns a node (a handler's node-set argument ran ~17%
/// slower). Callers convert through `From` on the failure path only.
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
        Error::new(error_class(), e.message())
    }
}

/* SAFETY: the buffer is only ever read or written while the GVL is held, which
 * serialises every Ruby thread. The Send bound comes from magnus's TypedData,
 * which has to assume a wrapped value may be freed on whichever thread runs the
 * GC - still under the GVL. Nothing here is shared without it. */
unsafe impl Send for NodeVec {}

impl NodeVec {
    const fn new() -> Self {
        NodeVec {
            ptr: core::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn as_slice(&self) -> &[*mut c_void] {
        if self.ptr.is_null() {
            return &[];
        }
        // SAFETY: ptr is non-null and holds `len` initialised elements.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Geometric growth, restated from `mkr_grow_capacity`: double from 8 until
    /// it covers `need`, falling back to exactly `need` when doubling would
    /// overshoot what the element size allows. So the only failure is `need`
    /// itself not fitting, which the caller has already bounded by
    /// `NODE_SET_MAX`.
    fn grow_capacity(cap: usize, need: usize) -> Option<usize> {
        crate::falloc::grow_capacity(cap, need, core::mem::size_of::<*mut c_void>())
    }

    /// Append one node. `Err` only for the size cap or a capacity overflow -
    /// allocation failure raises inside Ruby.
    fn push(&mut self, node: *mut c_void) -> Result<(), PushError> {
        if self.len >= NODE_SET_MAX {
            return Err(PushError::SizeLimit);
        }
        if self.len == self.cap {
            let new_cap =
                Self::grow_capacity(self.cap, self.len + 1).ok_or(PushError::CapacityOverflow)?;
            // SAFETY: ptr is either null or a live ruby_xmalloc'd block of
            // `cap` elements; xrealloc2 does the overflow-checked multiply.
            self.ptr = unsafe {
                rb_sys::ruby_xrealloc2(
                    self.ptr as *mut c_void,
                    new_cap as rb_sys::size_t,
                    core::mem::size_of::<*mut c_void>() as rb_sys::size_t,
                ) as *mut *mut c_void
            };
            self.cap = new_cap;
        }
        // SAFETY: len < cap after the growth above.
        unsafe { *self.ptr.add(self.len) = node };
        self.len += 1;
        Ok(())
    }
}

impl Drop for NodeVec {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: paired with the ruby_xrealloc2 above. Called from the GC's
            // free, where touching Ruby objects would be wrong but freeing our
            // own buffer is exactly what the C did.
            unsafe { rb_sys::ruby_xfree(self.ptr as *mut c_void) };
        }
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
struct NodeSet {
    /// The owning Document. `Opaque` is magnus's form for a Ruby value stored
    /// in a Rust struct: it cannot be read back without a `&Ruby` (so never off
    /// a Ruby thread), and it is `Mark`, which is what makes storing it sound -
    /// `mark` below is what the GC follows to reach it.
    document: Opaque<Value>,
    /// Decided once: the stored pointers are `mkr_xml_node_t*`, so they wrap as
    /// `Makiri::XML::*`.
    doc_is_xml: bool,
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
                    .cap
                    .saturating_mul(core::mem::size_of::<*mut c_void>()),
            ),
            Err(_) => base,
        }
    }
}

impl NodeSet {
    /// Borrow the contents, turning a borrow conflict into a Ruby error instead
    /// of a panic (which, under `panic = "abort"`, would end the process).
    fn read(&self) -> Result<std::cell::Ref<'_, NodeVec>, Error> {
        self.nodes
            .try_borrow()
            .map_err(|_| Error::new(error_class(), "node set is already in use"))
    }

    fn write(&self) -> Result<std::cell::RefMut<'_, NodeVec>, Error> {
        self.nodes
            .try_borrow_mut()
            .map_err(|_| Error::new(error_class(), "node set is already in use"))
    }

    fn document(&self, ruby: &Ruby) -> Value {
        ruby.get_inner(self.document)
    }

    /// A copy of the node pointers, for the paths that must not hold a borrow:
    /// `each` yields (the block can push), and the set operators push into a
    /// result while reading an operand that may be the same object.
    fn snapshot(&self, ruby: &Ruby) -> Result<(Vec<*mut c_void>, Value, bool), Error> {
        let nodes = self.read()?.as_slice().to_vec();
        Ok((nodes, self.document(ruby), self.doc_is_xml))
    }
}

/// Wrap a stored node, choosing the representation by the set's fixed document
/// kind. This is the ONLY place a stored pointer is cast back to a typed one,
/// and `doc_is_xml` is what justifies the cast.
unsafe fn wrap(node: *mut c_void, document: Value, doc_is_xml: bool) -> Value {
    let raw = if doc_is_xml {
        wrap_xml_node(node, document.as_raw())
    } else {
        wrap_html_node(node as *mut LxbNode, document.as_raw())
    };
    Value::from_raw(raw)
}

/// `Makiri::NodeSet`, as created by Init_makiri.
fn node_set_class() -> RClass {
    CLASS_NODE_SET.class()
}

/* ------------------------------------------------------------------ */
/* the C API other glue files call                                    */
/* ------------------------------------------------------------------ */

/// # Safety
/// `document` must be a live `Makiri::Document`.
pub unsafe fn node_set_new(document: VALUE) -> VALUE {
    let ruby = Ruby::get_unchecked();
    let doc = Value::from_raw(document);
    let doc_is_xml =
        rb_sys::rb_obj_is_kind_of(document, CLASS_XML_DOCUMENT.raw()) == rb_sys::Qtrue as VALUE;
    let obj = ruby
        .wrap(NodeSet {
            document: doc.into(),
            doc_is_xml,
            nodes: RefCell::new(NodeVec::new()),
        })
        .as_raw();
    /* While `wrap` allocates the object - a GC point - `document` is held only by
     * the boxed struct, where no mark sees it. Using it afterwards keeps it on
     * the machine stack across that call, pinned by the conservative scan, so
     * compaction cannot move it out from under the stored copy. */
    core::hint::black_box(document);
    obj
}

/// # Safety
/// `rb_set` must be a `Makiri::NodeSet`; `node` a node of its document.
///
/// `Err` for the fail-closed refusals: the size cap, a capacity overflow, a busy
/// set. Growing the array can still raise `NoMemoryError` from Ruby's allocator,
/// so a caller that owns something needing a drop pushes under `protect`.
#[inline]
pub unsafe fn node_set_push(rb_set: VALUE, node: *mut c_void) -> Result<(), PushError> {
    /* The hot path: one call per node of every CSS and XPath result. It uses
     * the unprotected accessor deliberately - magnus's `try_convert` costs an
     * rb_protect (a setjmp) per call, which measured ~26% off `Node#css`. See
     * the note on `typed_data_unprotected`. */
    let s: &NodeSet = typed_data_unprotected(rb_set);
    let Ok(mut nodes) = s.nodes.try_borrow_mut() else {
        return Err(PushError::Busy);
    };
    nodes.push(node)
}

/* ------------------------------------------------------------------ */
/* Ruby methods                                                       */
/* ------------------------------------------------------------------ */

fn length(rb_self: &NodeSet) -> Result<usize, Error> {
    Ok(rb_self.read()?.len())
}

/// A new NodeSet over `[beg, beg+len)` of `nodes` (already clamped).
fn slice_of(
    nodes: &[*mut c_void],
    document: Value,
    beg: usize,
    len: usize,
) -> Result<Value, Error> {
    let (result, r) = new_result(document)?;
    {
        let mut w = r.write()?;
        for n in &nodes[beg..beg + len] {
            w.push(*n)?;
        }
    }
    Ok(result)
}

/// `set[i]` -> Node or nil (negative counts from the end);
/// `set[start, length]` and `set[range]` -> a new NodeSet, nil when the start is
/// out of range. Mirrors `Array#[]`.
fn aref(ruby: &Ruby, rb_self: &NodeSet, args: &[Value]) -> Result<Value, Error> {
    let count = rb_self.read()?.len() as c_long;

    /* The single-index form is the common one and stays O(1): read the one
     * pointer under a short borrow, release it, then wrap. Wrapping allocates,
     * so the borrow must be gone first - not for marking (an immutable borrow
     * is re-entrant) but because the wrap can run arbitrary Ruby. The slicing
     * forms copy, since they build a whole new set anyway. */
    if args.len() == 1 && !args[0].is_kind_of(ruby.class_range()) {
        let mut i = c_long::try_convert(args[0])?;
        if i < 0 {
            i += count;
        }
        if i < 0 || i >= count {
            return Ok(ruby.qnil().as_value());
        }
        let node = rb_self.read()?.as_slice()[i as usize];
        return Ok(unsafe { wrap(node, rb_self.document(ruby), rb_self.doc_is_xml) });
    }

    let (nodes, document, _doc_is_xml) = rb_self.snapshot(ruby)?;

    if args.len() == 2 {
        let mut beg = c_long::try_convert(args[0])?;
        let mut len = c_long::try_convert(args[1])?;
        if beg < 0 {
            beg += count;
        }
        if beg < 0 || beg > count || len < 0 {
            return Ok(ruby.qnil().as_value());
        }
        if len > count - beg {
            len = count - beg;
        }
        return slice_of(&nodes, document, beg as usize, len as usize);
    }

    if args.len() != 1 {
        return Err(Error::new(
            ruby.exception_arg_error(),
            format!(
                "wrong number of arguments (given {}, expected 1..2)",
                args.len()
            ),
        ));
    }

    if args[0].is_kind_of(ruby.class_range()) {
        let (mut beg, mut len): (c_long, c_long) = (0, 0);
        // SAFETY: a Range, and count is its bound. err=0 means "return nil when
        // the start is out of range" rather than raising.
        let ok =
            unsafe { rb_sys::rb_range_beg_len(args[0].as_raw(), &mut beg, &mut len, count, 0) };
        if ok != rb_sys::Qtrue as VALUE {
            return Ok(ruby.qnil().as_value());
        }
        return slice_of(&nodes, document, beg as usize, len as usize);
    }

    /* Only a Range can reach here: the single-index form returned above. */
    Err(Error::new(
        ruby.exception_arg_error(),
        format!(
            "wrong number of arguments (given {}, expected 1..2)",
            args.len()
        ),
    ))
}

fn each(ruby: &Ruby, rb_self: &NodeSet) -> Result<Value, Error> {
    let this = unsafe { Value::from_raw(rb_self_value(rb_self)) };
    if !ruby.block_given() {
        return Ok(this.enumeratorize("each", ()).as_value());
    }
    /* Snapshot first: the block can call back into this set (even mutate it
     * through a query), and holding the borrow across the yield would turn that
     * into an error for no reason. Iterating a copy is also what makes
     * concurrent growth harmless rather than a dangling read. */
    let (nodes, document, doc_is_xml) = rb_self.snapshot(ruby)?;
    for n in nodes {
        let _: Value = ruby.yield_value(unsafe { wrap(n, document, doc_is_xml) })?;
    }
    Ok(this)
}

/// The receiver as a `VALUE`.
///
/// magnus hands methods a `&NodeSet`, not the object; `each` needs the object
/// itself both to return and to enumeratorize. The reference points into the
/// wrapped data, and `rb_typeddata_...` has no inverse, so the object is
/// recovered from the frame's receiver.
unsafe fn rb_self_value(_s: &NodeSet) -> VALUE {
    rb_sys::rb_current_receiver()
}

fn dup(ruby: &Ruby, rb_self: &NodeSet, _args: &[Value]) -> Result<Value, Error> {
    let document = rb_self.document(ruby);
    let mine = rb_self.read()?;
    slice_of(mine.as_slice(), document, 0, mine.len())
}

/* ---- set operations ----
 *
 * Results preserve encounter order (self first) and dedupe by node identity.
 * Document order is NOT imposed - that is the XPath engine's job; these mirror
 * Nokogiri's operators on the common same-query operands.
 *
 * None of these copies an operand. The borrows are held across the pushes into
 * the result, which is sound for two reasons: the operands are only read (two
 * immutable borrows, so `a | a` is fine), and the result is a fresh object, so
 * its mutable borrow is a different cell. A push can trigger a GC, and `mark`
 * takes no borrow - see the note on the struct. An earlier version snapshotted
 * each operand into a Vec to sidestep all this and measured about half the C's
 * throughput. */

/// Pointer hashing, matching `mkr_ptr_hash` (the MurmurHash3 fmix64 finalizer).
///
/// std's SipHash is the right default for attacker-chosen keys; these are heap
/// addresses, and paying for it measured about a third of the C's throughput on
/// the difference operator.
#[derive(Default, Clone, Copy)]
struct PtrHasher(u64);

impl core::hash::Hasher for PtrHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        /* Only ever fed one pointer, but stay correct if that changes. */
        for b in bytes {
            self.write_u8(*b);
        }
    }

    fn write_u8(&mut self, b: u8) {
        self.0 = self.0.rotate_left(8) ^ u64::from(b);
    }

    fn write_usize(&mut self, p: usize) {
        let mut h = p as u64;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        self.0 = h;
    }
}

type PtrBuild = core::hash::BuildHasherDefault<PtrHasher>;
type PtrSet = HashSet<*mut c_void, PtrBuild>;

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
    fn build(nodes: &[*mut c_void]) -> Index {
        if nodes.len() <= HASH_MIN {
            return Index::Linear;
        }
        let mut set = PtrSet::default();
        if set.mkr_reserve(nodes.len()).is_err() {
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
        if set.mkr_reserve(expected).is_err() {
            return Index::Linear;
        }
        Index::Hashed(set)
    }

    fn contains(&self, n: *mut c_void, fallback: &[*mut c_void]) -> bool {
        match self {
            Index::Hashed(s) => s.contains(&n),
            Index::Linear => fallback.contains(&n),
        }
    }

    /// True when `n` had not been seen. `already` is what the linear fallback
    /// scans - the result built so far.
    fn insert(&mut self, n: *mut c_void, already: &[*mut c_void]) -> bool {
        match self {
            Index::Hashed(s) => s.insert(n),
            Index::Linear => !already.contains(&n),
        }
    }
}

/// The other operand as a set, required to share `document`.
///
/// A result NodeSet borrows exactly one document (the GC keepalive) and one
/// representation flag to wrap its nodes, so mixing two documents - which also
/// means possibly mixing HTML and XML - would wrap a node under the wrong
/// representation and fail to keep its document alive. Fail closed rather than
/// produce a corrupt set.
fn other_of<'a>(ruby: &Ruby, document: Value, other: Value) -> Result<&'a NodeSet, Error> {
    if !other.is_kind_of(node_set_class()) {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::NodeSet",
        ));
    }
    let o = <&NodeSet>::try_convert(other)?;
    if o.document(ruby).as_raw() != document.as_raw() {
        return Err(Error::new(
            error_class(),
            "cannot combine node sets from different documents",
        ));
    }
    Ok(o)
}

/// An empty NodeSet over `document`, plus the handle to fill it through.
///
/// The reference's lifetime is unconstrained, as magnus's own `try_convert` for
/// a wrapped type gives: the data lives as long as the Ruby object, which the
/// returned `Value` keeps rooted on the caller's stack.
fn new_result<'a>(document: Value) -> Result<(Value, &'a NodeSet), Error> {
    let raw = unsafe { node_set_new(document.as_raw()) };
    /* Just built by the line above, so the type is known - no need to pay for
     * the checked conversion. */
    Ok((unsafe { Value::from_raw(raw) }, unsafe {
        typed_data_unprotected(raw)
    }))
}

/// `self | other` -> union, deduped, self first.
fn op_or(ruby: &Ruby, rb_self: &NodeSet, other: Value) -> Result<Value, Error> {
    let document = rb_self.document(ruby);
    let o = other_of(ruby, document, other)?;
    let mine = rb_self.read()?;
    let theirs = o.read()?;

    let (result, r) = new_result(document)?;
    let mut w = r.write()?;
    let mut seen = Index::empty(mine.len() + theirs.len());
    for &n in mine.as_slice().iter().chain(theirs.as_slice().iter()) {
        if seen.insert(n, w.as_slice()) {
            w.push(n)?;
        }
    }
    drop(w);
    Ok(result)
}

/// `self + other` -> concatenation, duplicates kept.
fn op_plus(ruby: &Ruby, rb_self: &NodeSet, other: Value) -> Result<Value, Error> {
    let document = rb_self.document(ruby);
    let o = other_of(ruby, document, other)?;
    let mine = rb_self.read()?;
    let theirs = o.read()?;

    let (result, r) = new_result(document)?;
    let mut w = r.write()?;
    for &n in mine.as_slice().iter().chain(theirs.as_slice().iter()) {
        w.push(n)?;
    }
    drop(w);
    Ok(result)
}

/// The shared core of `&` and `-`: keep each node of self whose membership in
/// other equals `keep_if_in_other`, deduped, in self's order.
fn op_filter(
    ruby: &Ruby,
    rb_self: &NodeSet,
    other: Value,
    keep_if_in_other: bool,
) -> Result<Value, Error> {
    let document = rb_self.document(ruby);
    let o = other_of(ruby, document, other)?;
    let mine = rb_self.read()?;
    let theirs = o.read()?;

    let theirs_index = Index::build(theirs.as_slice());
    let (result, r) = new_result(document)?;
    let mut w = r.write()?;
    let mut seen = Index::empty(mine.len());
    for &n in mine.as_slice() {
        if theirs_index.contains(n, theirs.as_slice()) != keep_if_in_other {
            continue;
        }
        if seen.insert(n, w.as_slice()) {
            w.push(n)?;
        }
    }
    drop(w);
    Ok(result)
}

fn op_and(ruby: &Ruby, rb_self: &NodeSet, other: Value) -> Result<Value, Error> {
    op_filter(ruby, rb_self, other, true)
}

fn op_minus(ruby: &Ruby, rb_self: &NodeSet, other: Value) -> Result<Value, Error> {
    op_filter(ruby, rb_self, other, false)
}

/// `NodeSet.new(document_or_node, list = [])`.
///
/// Mirrors Nokogiri: the first argument is the owning Document (or any node,
/// whose document is taken) that the set pins as a GC keepalive; the optional
/// list seeds it. Every listed node MUST belong to that document - one from
/// another document or representation would be re-wrapped under the wrong
/// document and kind, which is exactly the HTML/XML type confusion the set's
/// opaque storage relies on the document kind to avoid, so it is rejected.
fn s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let (ctx,) = a.required;
    let (list,) = a.optional;

    let doc_raw = unsafe {
        if rb_sys::rb_obj_is_kind_of(ctx.as_raw(), CLASS_DOCUMENT.raw()) == rb_sys::Qtrue as VALUE {
            ctx.as_raw()
        } else if rb_sys::rb_obj_is_kind_of(ctx.as_raw(), CLASS_NODE.raw())
            == rb_sys::Qtrue as VALUE
        {
            keepalive_document(ctx)?.as_raw()
        } else {
            return Err(Error::new(
                ruby.exception_type_error(),
                "expected a Makiri::Document or Node as the first argument",
            ));
        }
    };
    let document = unsafe { Value::from_raw(doc_raw) };

    let (set, s) = new_result(document)?;
    let Some(list) = list.filter(|v| !v.is_nil()) else {
        return Ok(set);
    };
    let Some(arr) = RArray::from_value(list) else {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected an Array of nodes as the second argument",
        ));
    };

    let mut w = s.write()?;
    for item in arr.into_iter() {
        let ok = unsafe {
            rb_sys::rb_obj_is_kind_of(item.as_raw(), CLASS_NODE.raw()) == rb_sys::Qtrue as VALUE
                && keepalive_document(item)?.as_raw() == doc_raw
        };
        if !ok {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "every node must be a Makiri node belonging to the given document",
            ));
        }
        w.push(node_raw(item)?)?;
    }
    drop(w);
    let _ = document;
    Ok(set)
}

/// # Safety
/// Called from `Init_makiri`, with the classes already defined.
pub unsafe extern "C" fn init_node_set() {
    let klass = node_set_class();

    /* Nodes come only from C; `.new` seeds through the factory below. */
    rb_sys::rb_undef_alloc_func(CLASS_NODE_SET.raw());
    klass
        .define_singleton_method("new", magnus::function!(s_new, -1))
        .expect("NodeSet.new");

    klass
        .define_method("|", method!(op_or, 1))
        .expect("NodeSet#|");
    klass
        .define_method("+", method!(op_plus, 1))
        .expect("NodeSet#+");
    klass
        .define_method("&", method!(op_and, 1))
        .expect("NodeSet#&");
    klass
        .define_method("-", method!(op_minus, 1))
        .expect("NodeSet#-");

    klass
        .define_method("length", method!(length, 0))
        .expect("NodeSet#length");
    klass
        .define_method("[]", method!(aref, -1))
        .expect("NodeSet#[]");
    klass
        .define_method("each", method!(each, 0))
        .expect("NodeSet#each");
    klass
        .define_method("dup", method!(dup, -1))
        .expect("NodeSet#dup");
    /* #clone is defined in Ruby (node_set.rb) so it can honour `freeze:`. */
}
