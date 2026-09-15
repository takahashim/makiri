//! The engine's runtime storage - node-sets, owned text, values and the
//! per-evaluate caches - one module each, re-exported under one namespace.
//!
//! These operate on the raw layouts in `xpath::value`; the guards in
//! `xpath::own` are the ownership-safe way to hold them.

#[path = "runtime_abi/cache.rs"]
pub mod cache;
#[path = "runtime_abi/nodeset.rs"]
pub mod nodeset;
#[path = "runtime_abi/text.rs"]
pub mod text;
#[path = "runtime_abi/value.rs"]
pub mod value;

pub use nodeset::{nodeset_clear, nodeset_init, nodeset_push};
pub use text::{owned_text_clear, owned_text_init};
pub use value::{val_clear, val_set_borrowed_text_copy, val_set_owned_text};
