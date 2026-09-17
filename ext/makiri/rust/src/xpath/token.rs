//! The engine's view of the opaque node token.
//!
//! The type itself lives at the crate root ([`crate::token`]) because the bridge
//! and the backends make and read tokens too, and only its `kind`-checked
//! resolvers may dereference one. This module re-exports it so the engine's own
//! files keep importing `super::token::Token`.

#![forbid(unsafe_code)]

pub use crate::token::{Kind, Token};
