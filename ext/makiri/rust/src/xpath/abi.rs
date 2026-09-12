//! The engine's view of the shared C types.
//!
//! They live at the crate root (`crate::xpath_abi`) because the glue needs them
//! as well; this re-export keeps every `super::abi::` in the engine working.

pub use crate::xpath_abi::*;
