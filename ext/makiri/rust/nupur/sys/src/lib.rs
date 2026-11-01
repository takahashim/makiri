//! `nupur_sys`: Ruby's C API as Nupur sees it - the one crate of the layer
//! that depends on rb-sys, so Nupur itself cannot name rb-sys.
//!
//! For now it re-exports rb-sys whole, and Nupur calls through it exactly as
//! it called rb-sys. The re-export changes no path the lints resolve (an item
//! is still `rb_sys::...` where it is defined). The later steps narrow what
//! this crate gives out to wrappers that take a proof of `rb_protect` and of
//! the GVL, so those rules become types rather than lints.
#![forbid(unsafe_code)]

pub use rb_sys::*;
