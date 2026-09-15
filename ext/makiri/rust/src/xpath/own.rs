//! Guards over the C allocations the engine passes around: owned text,
//! node-sets and values.
//!
//! The C frees each of these by hand at every bail - a `nodeset_clear` per
//! early return, a `owned_text_clear` per error path. That is where a leak
//! comes from, and it is what `Drop` exists to stop having to get right. One
//! module for all three, so a site that hand-writes the cleanup instead is
//! visibly the odd one out.

use super::abi::*;
use super::dom::Dom;
use core::ffi::c_void;
use core::ptr;

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
    /// `budget` must be null or live.
    pub unsafe fn push<'d, D: Dom<'d>>(
        &mut self,
        n: D::Node,
        budget: *mut Budget,
    ) -> Result<(), Reported> {
        nodeset_push(self.as_mut(), D::token(n), budget)
    }
    /// # Safety
    /// `i` must be below `count()`, and the set must hold `doc`'s handles.
    pub unsafe fn get<'d, D: Dom<'d>>(&self, doc: D, i: usize) -> D::Node {
        debug_assert!(i < self.0.count);
        doc.node(*self.0.items.add(i))
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
