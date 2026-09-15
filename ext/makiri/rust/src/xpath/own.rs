//! Guards over the three C allocations the engine passes around.
//!
//! The C frees each of these by hand at every bail - a `nodeset_clear` per
//! early return, a `owned_text_clear` per error path. That is where a leak
//! comes from, and it is what `Drop` exists to stop having to get right. One
//! module for all three, so a site that hand-writes the cleanup instead is
//! visibly the odd one out.

use super::abi::*;
use super::ast_ops::{node_free, step_clear};
use super::dom::Dom;
use crate::falloc::raw::mkr_reallocarray;
use core::ffi::c_void;
use core::mem::ManuallyDrop;
use core::ptr;
use core::ptr::NonNull;

/// An owned AST node: a compiled root, or a subtree still being built.
///
/// The AST keeps its C layout so the evaluator can borrow it, but ownership is
/// represented by Rust: dropping this frees the node and everything under it.
/// Raw AST pointers cross this type only through `from_raw` or `into_raw`.
pub(crate) struct Ast(NonNull<Node>);

impl Ast {
    /// # Safety
    /// `ptr` must be a live root AST allocated by `node_alloc`, and no
    /// other owner may free it after this call.
    pub(crate) unsafe fn from_raw(ptr: *mut Node) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    /// Borrow the AST for the evaluator. The evaluator mutates memo fields, so
    /// callers must keep the GVL/exclusive evaluation contract while using it.
    pub(crate) fn as_raw(&self) -> *mut Node {
        self.0.as_ptr()
    }

    /// Adopt a node the caller has just allocated.
    ///
    /// # Safety
    /// As [`Self::from_raw`], for a pointer already known to be non-null.
    pub(crate) unsafe fn from_non_null(ptr: NonNull<Node>) -> Self {
        Self(ptr)
    }

    /// The node's payload by kind, for filling it in while it is being built.
    ///
    /// # Safety
    /// Writes must keep the node in a state `node_free` can take apart: an
    /// owned child pointer is null or owned by this node alone.
    pub(crate) unsafe fn payload_mut(&mut self) -> NodeMut<'_> {
        Node::view_mut(self.0.as_ptr())
    }

    /// Transfer ownership to the legacy raw-pointer ABI.
    pub(crate) fn into_raw(self) -> *mut Node {
        let ptr = self.0.as_ptr();
        core::mem::forget(self);
        ptr
    }

    /// Release a raw AST at an ABI boundary.
    ///
    /// This is the only operation CSS/XPath construction code should need when
    /// it still has a legacy raw pointer. Keeping it beside `Ast::Drop` makes
    /// the C ownership rule explicit without spreading `node_free` calls
    /// across the builders.
    pub(crate) unsafe fn drop_raw(ptr: *mut Node) {
        if let Some(ast) = Self::from_raw(ptr) {
            drop(ast);
        }
    }
}

impl Drop for Ast {
    fn drop(&mut self) {
        // SAFETY: `Ast` is constructed only from an owned live AST root.
        unsafe { node_free(self.0.as_ptr()) }
    }
}

/// A step under construction. Dropping it clears the step - its name texts and
/// predicates - the way the AST destructor would.
pub(crate) struct OwnedStep(Step);

impl OwnedStep {
    /// An empty step: no name texts, no predicates.
    pub(crate) fn new(axis: u32, kind: u32) -> Self {
        Self(Step {
            axis,
            test: NodeTest {
                kind,
                prefix: TextSlot::empty(),
                local: TextSlot::empty(),
                pi_target: TextSlot::empty(),
            },
            predicates: ptr::null_mut(),
            npredicates: 0,
        })
    }

    fn into_raw(self) -> Step {
        ManuallyDrop::new(self).0
    }
}

impl core::ops::Deref for OwnedStep {
    type Target = Step;
    fn deref(&self) -> &Step {
        &self.0
    }
}

impl core::ops::DerefMut for OwnedStep {
    fn deref_mut(&mut self) -> &mut Step {
        &mut self.0
    }
}

impl Drop for OwnedStep {
    fn drop(&mut self) {
        // SAFETY: an `OwnedStep` owns every text and predicate it holds.
        unsafe { step_clear(&mut self.0) }
    }
}

/// A growable array in libc's allocator (through `falloc`), which is what the AST
/// destructors free.
///
/// No `Drop` of its own: freeing an element means something different for a
/// step and for a node, so [`StepArray`] and [`NodeArray`] each supply it.
struct RawArray<T> {
    v: *mut T,
    n: usize,
    cap: usize,
}

impl<T> RawArray<T> {
    const fn new() -> Self {
        Self {
            v: ptr::null_mut(),
            n: 0,
            cap: 0,
        }
    }

    /// Make room for one more element, growing geometrically. `false` on OOM,
    /// with the array unchanged.
    fn reserve_one(&mut self) -> bool {
        if self.n < self.cap {
            return true;
        }
        let Some(want) =
            crate::falloc::grow_capacity(self.cap, self.n + 1, core::mem::size_of::<T>())
        else {
            return false;
        };
        // SAFETY: `v` is null or this array's own C allocation.
        let p = unsafe {
            mkr_reallocarray(self.v as *mut c_void, want, core::mem::size_of::<T>()) as *mut T
        };
        if p.is_null() {
            return false;
        }
        self.v = p;
        self.cap = want;
        true
    }

    /// # Safety
    /// `reserve_one` must have succeeded since the last push.
    unsafe fn push_reserved(&mut self, item: T) {
        debug_assert!(self.n < self.cap);
        ptr::write(self.v.add(self.n), item);
        self.n += 1;
    }
}

/// The steps of a path under construction. Dropping it clears each step and
/// frees the array; installing hands both to the node.
pub(crate) struct StepArray(RawArray<Step>);

impl StepArray {
    pub(crate) const fn new() -> Self {
        Self(RawArray::new())
    }

    pub(crate) fn len(&self) -> usize {
        self.0.n
    }

    /// Append a finished step. On OOM the step comes back, and clears when it
    /// is dropped.
    pub(crate) fn try_push(&mut self, step: OwnedStep) -> Result<(), OwnedStep> {
        if !self.0.reserve_one() {
            return Err(step);
        }
        // SAFETY: reserved just above.
        unsafe { self.0.push_reserved(step.into_raw()) };
        Ok(())
    }

    /// # Safety
    /// `path` must be a live `NK_PATH` node with no steps yet.
    pub(crate) unsafe fn install_into_path(self, path: *mut Node) {
        let NodeMut::Path(p) = Node::view_mut(path) else {
            unreachable!("install_into_path on a non-PATH node")
        };
        let (steps, nsteps) = self.into_raw_parts();
        p.steps = steps;
        p.nsteps = nsteps;
    }

    /// # Safety
    /// `filter` must be a live `NK_FILTER` node with no trailing path yet.
    pub(crate) unsafe fn install_as_filter_path(self, filter: *mut Node) {
        let NodeMut::Filter(f) = Node::view_mut(filter) else {
            unreachable!("install_as_filter_path on a non-FILTER node")
        };
        let (steps, nsteps) = self.into_raw_parts();
        f.path_steps = steps;
        f.npath = nsteps;
    }

    fn into_raw_parts(self) -> (*mut Step, usize) {
        let this = ManuallyDrop::new(self);
        (this.0.v, this.0.n)
    }
}

impl Drop for StepArray {
    fn drop(&mut self) {
        // SAFETY: every entry below `n` is an owned step; `v` is the array's own.
        unsafe {
            for i in 0..self.0.n {
                step_clear(self.0.v.add(i));
            }
            free_c(self.0.v as *mut c_void);
        }
    }
}

/// Owned node pointers under construction - predicates or call arguments.
/// Dropping it frees each node and the array; installing hands both over.
pub(crate) struct NodeArray(RawArray<*mut Node>);

impl NodeArray {
    pub(crate) const fn new() -> Self {
        Self(RawArray::new())
    }

    pub(crate) fn len(&self) -> usize {
        self.0.n
    }

    /// Append a finished node. On OOM the node comes back, and frees when it is
    /// dropped.
    pub(crate) fn try_push(&mut self, node: Ast) -> Result<(), Ast> {
        if !self.0.reserve_one() {
            return Err(node);
        }
        // SAFETY: reserved just above.
        unsafe { self.0.push_reserved(node.into_raw()) };
        Ok(())
    }

    pub(crate) fn install_into_step(self, step: &mut Step) {
        let (predicates, npredicates) = self.into_raw_parts();
        step.predicates = predicates;
        step.npredicates = npredicates;
    }

    /// # Safety
    /// `call` must be a live `NK_FNCALL` node with no arguments yet.
    pub(crate) unsafe fn install_as_args(self, call: *mut Node) {
        let NodeMut::FnCall(c) = Node::view_mut(call) else {
            unreachable!("install_as_args on a non-FNCALL node")
        };
        let (args, nargs) = self.into_raw_parts();
        c.args = args;
        c.nargs = nargs;
    }

    /// # Safety
    /// `filter` must be a live `NK_FILTER` node with no predicates yet.
    pub(crate) unsafe fn install_as_filter_preds(self, filter: *mut Node) {
        let NodeMut::Filter(f) = Node::view_mut(filter) else {
            unreachable!("install_as_filter_preds on a non-FILTER node")
        };
        let (preds, npreds) = self.into_raw_parts();
        f.preds = preds;
        f.npreds = npreds;
    }

    fn into_raw_parts(self) -> (*mut *mut Node, usize) {
        let this = ManuallyDrop::new(self);
        (this.0.v, this.0.n)
    }
}

impl Drop for NodeArray {
    fn drop(&mut self) {
        // SAFETY: entries are null or owned nodes; `v` is the array's own.
        unsafe {
            for i in 0..self.0.n {
                Ast::drop_raw(*self.0.v.add(i));
            }
            free_c(self.0.v as *mut c_void);
        }
    }
}

extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}

/// The owner of a [`TextSlot`]: it frees the allocation on drop. Rust code that
/// holds engine text holds one of these; the slot is only the raw layout.
pub struct OwnedText(pub(crate) TextSlot);

impl OwnedText {
    pub fn new() -> OwnedText {
        OwnedText(TextSlot::empty())
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { self.0.as_bytes() }
    }
    pub(crate) fn from_slot(value: TextSlot) -> Self {
        Self(value)
    }
    pub(crate) fn is_absent(&self) -> bool {
        self.0.is_absent()
    }
    /// Hand the allocation to the caller; the guard is left empty.
    pub fn take(&mut self) -> TextSlot {
        core::mem::replace(&mut self.0, TextSlot::empty())
    }
}

impl Default for OwnedText {
    fn default() -> OwnedText {
        OwnedText::new()
    }
}

impl Drop for OwnedText {
    fn drop(&mut self) {
        unsafe { self.0.clear() }
    }
}

/// An owned `mkr_nodeset_t`.
pub struct Set(pub(crate) NodeSet);

const EMPTY_SET: NodeSet = NodeSet {
    items: ptr::null_mut(),
    count: 0,
    capacity: 0,
};

impl Set {
    pub fn new() -> Set {
        let mut ns = EMPTY_SET;
        unsafe { nodeset_init(&mut ns) };
        Set(ns)
    }
    /// Adopt a node-set the caller is handing over.
    pub fn adopt(ns: NodeSet) -> Set {
        Set(ns)
    }
    pub fn as_mut(&mut self) -> *mut NodeSet {
        &mut self.0
    }
    pub fn as_ptr(&self) -> *const NodeSet {
        &self.0
    }
    pub fn count(&self) -> usize {
        self.0.count
    }
    /// The node handles, in order.
    pub fn as_slice(&self) -> &[*mut c_void] {
        if self.0.count == 0 {
            &[]
        } else {
            // SAFETY: `items` holds `count` initialised handles.
            unsafe { core::slice::from_raw_parts(self.0.items, self.0.count) }
        }
    }
    pub fn take(&mut self) -> NodeSet {
        core::mem::replace(&mut self.0, EMPTY_SET)
    }
    /// Replace the contents, freeing what was there.
    pub fn replace(&mut self, ns: NodeSet) {
        unsafe { nodeset_clear(&mut self.0) };
        self.0 = ns;
    }
    /// # Safety
    /// `n` must be a live handle of the document being evaluated.
    pub unsafe fn push<D: Dom>(
        &mut self,
        n: D::Node,
        limits: *mut Limits,
        err: ErrSink,
    ) -> Result<(), Reported> {
        nodeset_push(self.as_mut(), D::to_void(n), limits, err)
    }
    /// # Safety
    /// `i` must be below `count()`, and the set must hold this backend's handles.
    pub unsafe fn get<D: Dom>(&self, i: usize) -> D::Node {
        debug_assert!(i < self.0.count);
        D::from_void(*self.0.items.add(i))
    }
}

impl Default for Set {
    fn default() -> Set {
        Set::new()
    }
}

impl Drop for Set {
    fn drop(&mut self) {
        unsafe { nodeset_clear(&mut self.0) }
    }
}

/// An owned `mkr_val_t`: dropping it clears the value.
///
/// Transparent, so a `[OwnedVal]` is the `mkr_val_t[]` a resolver reads.
#[repr(transparent)]
pub struct OwnedVal(pub(crate) Val);

impl OwnedVal {
    pub fn new() -> OwnedVal {
        OwnedVal(Val::EMPTY)
    }
    pub fn as_mut(&mut self) -> *mut Val {
        &mut self.0
    }
    pub fn as_ptr(&self) -> *const Val {
        &self.0
    }
    pub fn take(&mut self) -> Val {
        core::mem::replace(&mut self.0, Val::EMPTY)
    }
}

impl Default for OwnedVal {
    fn default() -> OwnedVal {
        OwnedVal::new()
    }
}

impl Drop for OwnedVal {
    fn drop(&mut self) {
        unsafe { val_clear(&mut self.0) }
    }
}

impl From<Val> for OwnedVal {
    /// Own `v`: dropping the owner clears it.
    fn from(v: Val) -> OwnedVal {
        OwnedVal(v)
    }
}

/// Read-only: a `DerefMut` would let a caller overwrite the value without
/// clearing it.
impl core::ops::Deref for OwnedVal {
    type Target = Val;
    fn deref(&self) -> &Val {
        &self.0
    }
}

impl OwnedVal {
    /// The node-set inside, for taking or replacing its array in place.
    pub fn as_nodeset_mut(&mut self) -> Option<&mut NodeSet> {
        self.0.as_nodeset_mut()
    }

    /// The values a slice of owners holds, for a callee that reads `&[Val]`.
    pub fn as_vals(owners: &[OwnedVal]) -> &[Val] {
        // SAFETY: `OwnedVal` is `repr(transparent)` over `Val`.
        unsafe { core::slice::from_raw_parts(owners.as_ptr() as *const Val, owners.len()) }
    }
}
