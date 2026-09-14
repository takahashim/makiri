//! Raw handle conversion shared by XPath DOM adapters.

#![allow(clippy::missing_safety_doc)]

/// Conversion boundary for the erased handles stored in the XPath ABI.
///
/// This is intentionally separate from the logical DOM contract in
/// [`super::dom`]. The HTML implementation converts live Lexbor pointers;
/// the XML implementation converts stamped [`NodeId`](crate::xml::NodeId)
/// tokens and validates them when performing DOM operations.
pub unsafe trait DomHandle {
    type Node: Copy + PartialEq;
    type Doc: Copy;

    fn null() -> Self::Node;
    fn is_null(n: Self::Node) -> bool;
    fn to_void(n: Self::Node) -> *mut core::ffi::c_void;

    /// `p` must be a handle produced by this backend, or null.
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node;
    /// `p` must be storage produced by this backend, or null.
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc;
}
