//! The XPath 1.0 front end, ported from ext/makiri/xpath/{lex,number,parse}.c
//! behind the same C ABI.
//!
//!   number.rs  the Number production: extent scan + conversion  (no unsafe)
//!   lex.rs     the tokenizer                                    (no unsafe)
//!   parse.rs   recursive descent, building the C AST            (unsafe: writes C nodes)
//!   abi.rs     the C types and the C functions we call back into
//!
//! What stays in C: the AST allocator / free (`mkr_node_alloc`, `mkr_node_free`)
//! and the post-parse passes (`mkr_apply_peephole`,
//! `mkr_mark_context_independent`). The CSS lowering builds the same AST through
//! the same allocator, so leaving allocation in one place keeps `mkr_css.c`
//! untouched and makes either side able to free what the other built.

pub mod abi;
pub mod lex;
pub mod number;
pub mod parse;
