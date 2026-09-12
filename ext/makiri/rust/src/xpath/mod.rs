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
//! The driver (`xpath-driver`), which mkr_xpath.c holds today:
//!   ctx.rs        the context, its registries and the evaluate entries
//!   limits.rs     the per-evaluate budgets
//!
//! The engine (`xpath-engine`), generic over `Dom`:
//!   dom.rs        the node-access contract, as a trait
//!   own.rs        guards over the C allocations the engine passes around
//!   ast.rs        the C AST's arrays, viewed as slices
//!   ast_ops.rs    building, destroying and rewriting one
//!   shared.rs     node-sets, owned text, values, the per-evaluate caches
//!   axis.rs       the thirteen axes, as orders over the tree
//!   order.rs      document order and its per-evaluate index
//!   value.rs      string-values, coercions, the string-value cache
//!   nodetest.rs   does a node match a step's test?
//!   attr_pred.rs  the [@name] / [@name='lit'] predicate shapes
//!   step_index.rs the //tag and //tag[N] index fast paths
//!   funcs.rs      the built-in function library
//!   eval.rs       node tests, predicates, steps, operators
//!
//! An instance binds the contract to one representation and exports the two
//! entry points the driver dispatches on. They are independent features, so
//! either C instance can be replaced on its own:
//!   dom_xml.rs / ffi_xml.rs      (`xpath-xml`)
//!   dom_html.rs / ffi_html.rs / html_abi.rs   (`xpath-html`)
//!
//! What stays in C: the AST allocator / free (`mkr_node_alloc`, `mkr_node_free`)
//! and the post-parse passes (`mkr_apply_peephole`,
//! `mkr_mark_context_independent`). The CSS lowering builds the same AST through
//! the same allocator, so leaving allocation in one place keeps `mkr_css.c`
//! untouched and makes either side able to free what the other built.

pub mod abi;
pub mod msg;

/* The front end (the `xpath` feature). */
pub mod ast;
pub mod lex;
pub mod number;

#[cfg(kani)]
mod verify;
pub mod parse;

/* The generic engine (the `xpath-engine` feature). A cargo feature is what
 * keeps the archive free of symbols the C files it replaces still define, so
 * the split follows the C translation units, not the Rust module tree. */
/* The driver (`xpath-driver`): the context, the budgets, and the two evaluate
 * entries. Independent of the instances - it dispatches to whichever language
 * provides each. */
#[cfg(feature = "xpath-driver")]
pub mod ctx;
#[cfg(feature = "xpath-driver")]
pub mod limits;

/* The shared primitives (`xpath-shared`), which ast.rs and shared.rs hold. The
 * views in ast.rs cost nothing and export nothing, so they come with the front
 * end; the operations beside them are gated. */
#[cfg(feature = "xpath-shared")]
pub mod ast_ops;
#[cfg(feature = "xpath-shared")]
pub mod shared;
#[cfg(feature = "xpath-engine")]
pub mod attr_pred;
#[cfg(feature = "xpath-engine")]
pub mod axis;
#[cfg(feature = "xpath-engine")]
pub mod dom;
#[cfg(feature = "xpath-engine")]
pub mod eval;
#[cfg(feature = "xpath-engine")]
pub mod funcs;
#[cfg(feature = "xpath-engine")]
pub mod nodetest;
#[cfg(feature = "xpath-engine")]
pub mod order;
#[cfg(feature = "xpath-engine")]
pub mod own;
#[cfg(feature = "xpath-engine")]
pub mod step_index;
#[cfg(feature = "xpath-engine")]
pub mod value;

/* The XML instance (`xpath-xml`). */
#[cfg(feature = "xpath-xml")]
pub mod dom_xml;
#[cfg(feature = "xpath-xml")]
pub mod ffi_xml;

/* The HTML instance (`xpath-html`). */
#[cfg(feature = "xpath-html")]
pub mod dom_html;
#[cfg(feature = "xpath-html")]
pub mod ffi_html;
#[cfg(feature = "xpath-html")]
pub mod html_abi;
