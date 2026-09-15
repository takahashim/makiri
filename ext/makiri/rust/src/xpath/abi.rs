//! The engine's prelude: the names nearly every engine module needs, gathered
//! from the modules that define them so a module can `use super::abi::*`.
//! It defines nothing.

pub use super::ast::*;
pub use super::ctx::{Handler, ResolverCall};
pub use super::funcs::{FN_OF_TYPE_POS, FN_OF_TYPE_POS_LAST};
pub use super::limits::Limits;
pub use super::msg::{
    XP_ERR_INTERNAL, XP_ERR_LIMIT, XP_ERR_NOT_IMPLEMENTED, XP_ERR_OOM, XP_ERR_RUNTIME,
    XP_ERR_SYNTAX, XP_ERR_TYPE, XP_OK,
};
pub use super::order::OrderIndex;
pub use super::runtime_abi::cache::{ptr_hash, StrCache, TextId};
pub use super::value::{
    NodeSet, TextSlot, Val, ValRef, ValU, T_BOOLEAN, T_NODESET, T_NUMBER, T_STRING,
};
pub use crate::cbuf::BUF_OK;

pub use super::ctx::ctx_document;
pub use super::ctx::ctx_limits;
pub use super::ctx::ctx_node;
pub use super::ctx::ctx_unprefixed_lax;
pub use super::ctx::Context;
pub use super::ctx::XPathValue;
pub use super::ctx::{ctx_backend, Backend};
pub use super::limits::limit_ast_node;
pub use super::limits::limit_check_expr_bytes;
pub use super::limits::limit_check_func_args;
pub use super::limits::limit_check_nodeset_size;
pub use super::limits::limit_check_predicates;
pub use super::limits::limit_check_steps;
pub use super::limits::limit_check_string_bytes;
pub use super::limits::limit_eval_op;
pub use super::limits::limit_recurse_enter;
pub use super::limits::limit_recurse_leave;
pub use super::limits::{budget_sink, Budget};
pub use super::msg::{ErrSink, Error, Reported};
pub use super::runtime_abi::nodeset_clear;
pub use super::runtime_abi::nodeset_init;
pub use super::runtime_abi::nodeset_push;
pub use super::runtime_abi::owned_text_clear;
pub use super::runtime_abi::val_clear;
pub use super::runtime_abi::val_set_owned_text;
pub(crate) use crate::cbuf::{buf_append, Buf};
pub use crate::falloc::calloc::grow_reserve;
pub use crate::falloc::calloc::strndup;
pub use crate::text::{BorrowedText, VerifiedText};
