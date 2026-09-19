//! Compatibility facade for the C allocator surface.
//!
//! The `mkr_*` names are retained where a raw pointer must cross the C ABI.
//! Implementations are split by responsibility: [`raw`] owns libc allocation,
//! [`cstr`] owns NUL-terminated C strings, and [`inject`] owns test-only OOM
//! injection. New Rust code should prefer the fallible typed APIs in the
//! parent [`crate::falloc`] module.

#![forbid(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

pub use super::cstr::{str_alloc, strdup, strndup};
pub use super::raw::grow_reserve;

#[cfg(feature = "alloc-inject")]
pub use super::inject::{alloc_inject_arm, alloc_inject_call_count, alloc_inject_should_fail};
