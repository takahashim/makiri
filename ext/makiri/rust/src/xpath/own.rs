//! Guards over the three C allocations the engine passes around.
//!
//! The C frees each of these by hand at every bail - a `mkr_nodeset_clear` per
//! early return, a `mkr_owned_text_clear` per error path. That is where a leak
//! comes from, and it is what `Drop` exists to stop having to get right. One
//! module for all three, so a site that hand-writes the cleanup instead is
//! visibly the odd one out.

use super::abi::*;
use super::dom::Dom;
use core::ptr;

/// An owned `mkr_owned_text_t`.
pub struct Text(pub OwnedText);

impl Text {
    pub fn new() -> Text {
        Text(OwnedText {
            ptr: ptr::null_mut(),
            len: 0,
        })
    }
    pub fn as_slice(&self) -> &[u8] {
        if self.0.ptr.is_null() || self.0.len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.0.ptr as *const u8, self.0.len) }
        }
    }
    pub fn as_mut(&mut self) -> *mut OwnedText {
        &mut self.0
    }
    /// Hand the allocation to the caller; the guard is left empty.
    pub fn take(&mut self) -> OwnedText {
        core::mem::replace(
            &mut self.0,
            OwnedText {
                ptr: ptr::null_mut(),
                len: 0,
            },
        )
    }
}

impl Default for Text {
    fn default() -> Text {
        Text::new()
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        unsafe { mkr_owned_text_clear(&mut self.0) }
    }
}

/// An owned `mkr_nodeset_t`.
pub struct Set(pub NodeSet);

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
pub struct OwnedVal(pub Val);

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
