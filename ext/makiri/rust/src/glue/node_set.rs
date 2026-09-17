//! `Makiri::NodeSet` (glue/ruby_node_set.c).
//!
//! The wrapper type - its private layout, Ruby-allocator storage and GC
//! `mark`/`free`, the C-facing `node_set_new` / `node_set_push`, and the Ruby
//! methods - lives in the bridge ([`crate::bridge::node_set`]), beside the
//! other raw-Ruby-ABI seams, so that glue-side callers can collect nodes with
//! the safe [`crate::bridge::node_set::Fill`] handle. This module re-exports it
//! for `Init_makiri` and the glue files that still import through `glue::abi`.

#![forbid(unsafe_code)]

pub use crate::bridge::node_set::{
    init_node_set, node_set_new, node_set_push, node_set_with_fill, Fill, PushError,
};
