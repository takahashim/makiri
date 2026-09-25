//! The engine's prelude: the names nearly every engine module needs, gathered
//! from the modules that define them so a module can `use super::abi::*`.
//! It defines nothing.

#![forbid(unsafe_code)]

pub use super::ast::*;
pub use super::ctx::{Names, Resolver, ResolverCall};
pub use super::funcs::{FN_CHILD_POS, FN_CHILD_POS_LAST, FN_OF_TYPE_POS, FN_OF_TYPE_POS_LAST};
pub use super::limits::Limits;
pub use super::order::OrderIndex;
pub use super::str_cache::{NodeText, StrCache, TextId};
pub use super::value::{NodeSet, Text, Val, ValRef};
pub use crate::engine_error::ErrorKind;

pub use super::ctx::XPathValue;
pub use super::ctx::{Context, Session};
pub use super::limits::Budget;
pub(crate) use crate::cbuf::{Buf, BufError};
pub use crate::engine_error::{ErrSink, Error, Reported};
pub use crate::text::VerifiedText;
