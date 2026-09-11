//! The XPath 1.0 front end, ported from ext/makiri/xpath/{lex,number,parse}.c
//! behind the same C ABI.
//!
//!   number.rs  the Number production: extent scan + conversion  (no unsafe)
//!   lex.rs     the tokenizer                                    (no unsafe)
//!   parse.rs   recursive descent, building the C AST            (unsafe: writes C nodes)
//!   abi.rs     the C types and the C functions we call back into
//!   dom.rs     the node-access contract as a trait, and its backends
//!   value.rs   string-values, coercions, document order         (generic over Dom)
//!   funcs.rs   the built-in function library                    (generic over Dom)
//!   eval.rs    axes, node tests, predicates, operators          (generic over Dom)
//!   ffi_xml.rs the XML instance's two exported entry points
//!
//! What stays in C: the AST allocator / free (`mkr_node_alloc`, `mkr_node_free`)
//! and the post-parse passes (`mkr_apply_peephole`,
//! `mkr_mark_context_independent`). The CSS lowering builds the same AST through
//! the same allocator, so leaving allocation in one place keeps `mkr_css.c`
//! untouched and makes either side able to free what the other built.

pub mod abi;

/* The front end (the `xpath` feature). */
pub mod lex;
pub mod number;
pub mod parse;

/* The engine (the `xpath-xml` feature). A cargo feature is what keeps the
 * archive free of symbols the C files it replaces still define, so the split
 * follows the C translation units, not the Rust module tree. */
#[cfg(feature = "xpath-xml")]
pub mod dom;
#[cfg(feature = "xpath-xml")]
pub mod eval;
#[cfg(feature = "xpath-xml")]
pub mod ffi_xml;
#[cfg(feature = "xpath-xml")]
pub mod funcs;
#[cfg(feature = "xpath-xml")]
pub mod value;
