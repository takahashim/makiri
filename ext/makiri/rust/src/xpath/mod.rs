//! The XPath 1.0 engine.
//!
//! Shared:
//!   abi.rs        the engine's prelude: the shared names, re-exported
//!   msg.rs        error messages, assembled without allocating
//!
//! The front end:
//!   number.rs     the Number production, read and written
//!   lex.rs        the tokenizer
//!   parse.rs      recursive descent
//!
//! The driver:
//!   ctx.rs        the context, its registries, and the evaluate entries
//!   limits.rs     the per-evaluate budgets
//!
//! The engine, generic over `Dom`:
//!   dom.rs        the node-access contract, as a trait
//!   ast.rs        the compiled AST
//!   ast_ops.rs    the peephole and hoisting pass over a parsed one
//!   str_cache.rs  the per-evaluate string-value cache
//!   axis.rs       the thirteen axes, as orders over the tree
//!   order.rs      document order and its per-evaluate index
//!   value.rs      the values, string-values and coercions
//!   nodetest.rs   does a node match a step's test?
//!   attr_pred.rs  the [@name] / [@name='lit'] predicate shapes
//!   step_index.rs the //tag and //tag[N] index fast paths
//!   funcs/        the built-in function library (ext.rs: Nokogiri and CSS hooks)
//!   eval.rs       predicates, steps, operators
//!
//! An instance binds the contract to one representation, and lives outside this
//! module so the engine stays representation-free:
//!   `lexbor::xpath`  Lexbor's nodes, through `lexbor::adapter::html`
//!   `xml::xpath`     the XML reader's nodes

#![allow(private_bounds)]
#![forbid(unsafe_code)]

pub mod abi;
pub mod msg;

pub mod ast;
pub mod lex;
pub mod number;
pub mod parse;

#[cfg(kani)]
mod verify;

#[cfg(test)]
mod tests;

pub mod ctx;
pub mod limits;

pub mod ast_ops;
pub mod str_cache;

pub mod attr_pred;
pub mod axis;
pub mod dom;
pub mod eval;
pub mod funcs;
pub mod nodetest;
pub mod order;
pub mod step_index;
pub mod value;
