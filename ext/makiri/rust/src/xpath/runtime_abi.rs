//! Compatibility facade for the feature-specific runtime ABI modules.
//!
//! The C-facing names remain in one Rust namespace, while ownership and cache
//! responsibilities live in separate modules.

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
pub use text::{
    mkr_borrowed_text_eq, mkr_owned_text_clear, mkr_owned_text_from_borrowed_copy,
    mkr_owned_text_init,
};
pub use value::{mkr_val_clear, mkr_val_set_borrowed_text_copy, mkr_val_set_owned_text};
