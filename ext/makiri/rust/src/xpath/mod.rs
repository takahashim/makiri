//! The XPath 1.0 engine.
//!
//! Shared:
//!   abi.rs        the engine's prelude: the shared names, re-exported
//!   msg.rs        error messages, assembled without allocating
//!
//! The front end:
//!   number.rs     the Number production, read and written       (no unsafe)
//!   lex.rs        the tokenizer                                 (no unsafe)
//!   parse.rs      recursive descent
//!
//! The driver:
//!   ctx.rs        the context, its registries and state
//!   evaluate.rs  the compiled-expression evaluation entries
//!   limits.rs     the per-evaluate budgets
//!
//! The engine, generic over `Dom`:
//!   dom.rs        the node-access contract, as a trait
//!   dom_handle.rs raw handle conversion
//!   own.rs        guards over the allocations the engine passes around
//!   ast.rs        the compiled AST's kinds and node layout
//!   ast_view.rs   the AST's arrays, viewed as slices
//!   ast_ops.rs    building, destroying and rewriting one
//!   runtime_abi.rs raw node-sets, owned text, values and per-evaluate caches
//!   axis.rs       the thirteen axes, as orders over the tree
//!   order.rs      document order and its per-evaluate index
//!   value.rs      string-values, coercions, the string-value cache
//!   nodetest.rs   does a node match a step's test?
//!   attr_pred.rs  the [@name] / [@name='lit'] predicate shapes
//!   step_index.rs the //tag and //tag[N] index fast paths
//!   funcs.rs      the built-in function library
//!   eval.rs       node tests, predicates, steps, operators
//!
//! An instance binds the contract to one representation:
//!   dom_xml.rs / ffi_xml.rs                    the XML reader's nodes
//!   dom_html.rs / ffi_html.rs / lexbor_abi.rs    Lexbor's nodes (`lexbor`)

#![allow(private_bounds)]

pub mod abi;
pub mod msg;

pub mod ast;
pub mod ast_view;
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
pub mod runtime_abi;

pub mod attr_pred;
pub mod axis;
pub mod dom;
pub mod dom_handle;
pub mod eval;
pub mod funcs;
pub mod nodetest;
pub mod order;
pub mod own;
pub mod step_index;
pub mod value;

/* The XML instance. */
pub mod dom_xml;
pub mod ffi_xml;

/* The HTML instance: it reads Lexbor's DOM, so it comes with `lexbor`. */
#[cfg(feature = "lexbor")]
pub mod dom_html;
#[cfg(feature = "lexbor")]
pub mod ffi_html;
#[cfg(feature = "lexbor")]
pub mod lexbor_abi;
