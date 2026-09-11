//! The XPath 1.0 engine, ported from ext/makiri/xpath/ behind the same C ABI.
//!
//! Shared:
//!   abi.rs        the C types and the C functions we call back into
//!   msg.rs        error messages, assembled without allocating
//!
//! The front end (`xpath`), which builds the C AST:
//!   number.rs     the Number production, read and written       (no unsafe)
//!   lex.rs        the tokenizer                                 (no unsafe)
//!   parse.rs      recursive descent                             (writes C nodes)
//!
//! The engine (`xpath-xml`), generic over `Dom`:
//!   dom.rs        the node-access contract as a trait, and its backends
//!   own.rs        guards over the C allocations the engine passes around
//!   ast.rs        the C AST's arrays, viewed as slices
//!   axis.rs       the thirteen axes, as orders over the tree
//!   order.rs      document order and its per-evaluate index
//!   value.rs      string-values, coercions, the string-value cache
//!   nodetest.rs   does a node match a step's test?
//!   attr_pred.rs  the [@name] / [@name='lit'] predicate shapes
//!   step_index.rs the //tag and //tag[N] index fast paths
//!   funcs.rs      the built-in function library
//!   eval.rs       node tests, predicates, steps, operators
//!   ffi_xml.rs    the XML instance's two exported entry points
//!
//! What stays in C: the AST allocator / free (`mkr_node_alloc`, `mkr_node_free`)
//! and the post-parse passes (`mkr_apply_peephole`,
//! `mkr_mark_context_independent`). The CSS lowering builds the same AST through
//! the same allocator, so leaving allocation in one place keeps `mkr_css.c`
//! untouched and makes either side able to free what the other built.

pub mod abi;
pub mod msg;

/* The front end (the `xpath` feature). */
pub mod lex;
pub mod number;
pub mod parse;

/* The engine (the `xpath-xml` feature). A cargo feature is what keeps the
 * archive free of symbols the C files it replaces still define, so the split
 * follows the C translation units, not the Rust module tree. */
#[cfg(feature = "xpath-xml")]
pub mod ast;
#[cfg(feature = "xpath-xml")]
pub mod attr_pred;
#[cfg(feature = "xpath-xml")]
pub mod axis;
#[cfg(feature = "xpath-xml")]
pub mod dom;
#[cfg(feature = "xpath-xml")]
pub mod eval;
#[cfg(feature = "xpath-xml")]
pub mod ffi_xml;
#[cfg(feature = "xpath-xml")]
pub mod funcs;
#[cfg(feature = "xpath-xml")]
pub mod nodetest;
#[cfg(feature = "xpath-xml")]
pub mod order;
#[cfg(feature = "xpath-xml")]
pub mod own;
#[cfg(feature = "xpath-xml")]
pub mod step_index;
#[cfg(feature = "xpath-xml")]
pub mod value;
