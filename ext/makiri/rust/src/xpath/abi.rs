//! Compatibility view of the shared XPath ABI types.
//!
//! The canonical definitions live at `crate::xpath_abi` because the Ruby glue
//! uses them too. This module keeps the engine's existing `super::abi` imports
//! local and makes the boundary role explicit; it does not define engine
//! models or DOM access.

pub use crate::xpath_abi::*;
