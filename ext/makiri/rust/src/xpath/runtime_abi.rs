//! The engine's runtime storage - node-sets, owned text, values and the
//! per-evaluate caches - one module each, re-exported under one namespace.
//!
//! These operate on the raw layouts in `crate::xpath_abi`; the guards in
//! `xpath::own` are the ownership-safe way to hold them.

#[path = "runtime_abi/cache.rs"]
pub mod cache;
#[path = "runtime_abi/nodeset.rs"]
pub mod nodeset;
#[path = "runtime_abi/text.rs"]
pub mod text;
#[path = "runtime_abi/value.rs"]
pub mod value;

pub use cache::{
    mkr_doc_order_index_clear, mkr_doc_order_index_init, mkr_str_cache_clear,
    mkr_str_cache_index_put, mkr_str_cache_init, mkr_str_cache_reindex, mkr_str_cache_truncate,
};
pub use nodeset::{mkr_nodeset_clear, mkr_nodeset_init, mkr_nodeset_push};
pub use text::{mkr_owned_text_clear, mkr_owned_text_init};
pub use value::{mkr_val_clear, mkr_val_set_borrowed_text_copy, mkr_val_set_owned_text};
