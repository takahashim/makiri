//! Compatibility facade for the C allocator surface.
//!
//! The `mkr_*` names are retained where a raw pointer must cross the C ABI.
//! Implementations are split by responsibility: [`raw`] owns libc allocation,
//! [`cstr`] owns NUL-terminated C strings, and [`inject`] owns test-only OOM
//! injection. New Rust code should prefer the fallible typed APIs in the
//! parent [`crate::falloc`] module.

#![allow(clippy::missing_safety_doc)]

pub use super::cstr::{mkr_str_alloc, mkr_strdup, mkr_strndup};
pub use super::raw::mkr_grow_reserve;

#[cfg(feature = "alloc-inject")]
pub use super::inject::{
    mkr_alloc_inject_arm, mkr_alloc_inject_calls, mkr_alloc_inject_should_fail,
};
