//! Guards over the three C allocations the engine passes around.
//!
//! The C frees each of these by hand at every bail - a `mkr_nodeset_clear` per
//! early return, a `mkr_owned_text_clear` per error path. That is where a leak
//! comes from, and it is what `Drop` exists to stop having to get right. One
//! module for all three, so a site that hand-writes the cleanup instead is
//! visibly the odd one out.

use super::abi::*;
use super::ast_ops::mkr_node_free;
use super::dom::Dom;
use core::ptr;
use core::ptr::NonNull;

/// An owned compiled XPath AST.
///
/// The AST still uses the stable C layout internally so the evaluator and the
/// ABI adapters can borrow it, but ownership is represented by Rust. Raw AST
/// pointers must cross this type only through `from_raw` or `into_raw`.
#[allow(dead_code)]
pub(crate) struct Ast(NonNull<Node>);

#[allow(dead_code)]
impl Ast {
    /// # Safety
    /// `ptr` must be a live root AST allocated by `mkr_node_alloc`, and no
    /// other owner may free it after this call.
    pub(crate) unsafe fn from_raw(ptr: *mut Node) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    /// Borrow the AST for the evaluator. The evaluator mutates memo fields, so
    /// callers must keep the GVL/exclusive evaluation contract while using it.
    pub(crate) fn as_raw(&self) -> *mut Node {
        self.0.as_ptr()
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
    /// the C ownership rule explicit without spreading `mkr_node_free` calls
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
        unsafe { mkr_node_free(self.0.as_ptr()) }
    }
}

/// An owned `mkr_owned_text_t`.
pub struct Text(pub(crate) OwnedText);

impl Text {
    pub fn new() -> Text {
        Text(OwnedText::empty())
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { self.0.as_bytes() }
    }
    pub(crate) fn from_owned(value: OwnedText) -> Self {
        Self(value)
    }
    pub(crate) fn is_absent(&self) -> bool {
        self.0.is_absent()
    }
    pub(crate) unsafe fn as_verified(&self) -> VerifiedText {
        VerifiedText::from_raw_parts(self.0.as_ptr(), self.0.len())
    }
    pub(crate) fn as_mut(&mut self) -> *mut OwnedText {
        &mut self.0
    }
    /// Hand the allocation to the caller; the guard is left empty.
    pub fn take(&mut self) -> OwnedText {
        core::mem::replace(&mut self.0, OwnedText::empty())
    }
}

impl Default for Text {
    fn default() -> Text {
        Text::new()
    }
}

impl Drop for Text {
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
        unsafe { mkr_nodeset_init(&mut ns) };
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
    pub fn take(&mut self) -> NodeSet {
        core::mem::replace(&mut self.0, EMPTY_SET)
    }
    /// Replace the contents, freeing what was there.
    pub fn replace(&mut self, ns: NodeSet) {
        unsafe { mkr_nodeset_clear(&mut self.0) };
        self.0 = ns;
    }
    /// # Safety
    /// `n` must be a live handle of the document being evaluated.
    pub unsafe fn push<D: Dom>(
        &mut self,
        n: D::Node,
        limits: *mut Limits,
        err: *mut Error,
    ) -> bool {
        mkr_nodeset_push(self.as_mut(), D::to_void(n), limits, err) == 0
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
        unsafe { mkr_nodeset_clear(&mut self.0) }
    }
}

/// An owned `mkr_val_t`.
pub struct OwnedVal(pub(crate) Val);

impl OwnedVal {
    pub fn new() -> OwnedVal {
        OwnedVal(Val {
            type_: 0,
            u: ValU { nodeset: EMPTY_SET },
        })
    }
    pub fn as_mut(&mut self) -> *mut Val {
        &mut self.0
    }
    pub fn as_ptr(&self) -> *const Val {
        &self.0
    }
    pub fn take(&mut self) -> Val {
        core::mem::replace(
            &mut self.0,
            Val {
                type_: 0,
                u: ValU { nodeset: EMPTY_SET },
            },
        )
    }
}

impl Default for OwnedVal {
    fn default() -> OwnedVal {
        OwnedVal::new()
    }
}

impl Drop for OwnedVal {
    fn drop(&mut self) {
        unsafe { mkr_val_clear(&mut self.0) }
    }
}
