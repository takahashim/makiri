//! Shared entry points and helpers for the targets.
//!
//! The targets go through the engine's front door, the one the Ruby glue uses:
//! a [`Context`] over a document, [`parse_owned`] against the context's
//! [`Budget`], and `evaluate` / `evaluate_first` returning an owned
//! [`XPathValue`]. Every handle is owned, so an early return frees it.

// Each target is its own crate and uses only part of this module.
#![allow(dead_code, unused_imports)]

use core::ffi::c_void;

pub use makiri::text::VerifiedText;
pub use makiri::token::Token;
pub use makiri::xml::tree::parse as xml_parse;
pub use makiri::xml::Document;
pub use makiri::xpath::ast::Ast;
pub use makiri::xpath::ctx::{Context, XPathValue};
pub use makiri::xpath::dom::Dom;
pub use makiri::xpath::limits::Budget;
pub use makiri::xpath::limits::Limits;
pub use makiri::xpath::parse::parse_owned;

/// A context over `doc`, rooted at its document node and pinned to the XML
/// engine - the same arguments the glue's `build_ctx` passes. `None` when the
/// document has no root.
pub fn xml_context(doc: &Document) -> Option<Context<'_, &Document>> {
    if doc.doc_node().is_invalid() {
        return None;
    }
    Some(makiri::xml::xpath::context(doc, doc.doc_node()))
}

/// Parse `text` under the context's caps, on a budget of the parse's own - the
/// way the glue parses.
pub fn parse<'d, D: Dom<'d>>(ctx: &Context<'d, D>, text: VerifiedText) -> Option<Box<Ast>> {
    let mut budget = Budget::with_limits(ctx.limits());
    parse_owned(text, &mut budget).ok()
}

/// Evaluate `ast` both ways the glue does, and hold the `at_xpath` fast path to
/// its promise: when both succeed with a node-set, `evaluate_first` answers the
/// first node of what `evaluate` answers. A failure on either side is not
/// compared - the fast path is allowed to finish a walk the full evaluator
/// overruns.
pub fn evaluate_both<'d, D: Dom<'d>>(ctx: &Context<'d, D>, ast: &Ast) {
    let full = ctx.evaluate(ast, None);
    let first = ctx.evaluate_first(ast, None);
    if let (Ok(XPathValue::NodeSet(all)), Ok(XPathValue::NodeSet(one))) = (&full, &first) {
        assert_eq!(
            all.as_slice().first(),
            one.as_slice().first(),
            "evaluate_first disagrees with evaluate"
        );
    }
}

/// An expression, owned for the call: the bytes up to the first NUL.
///
/// The engine takes a [`VerifiedText`] - valid UTF-8, no NUL - which the Ruby
/// bridge enforces before any expression reaches it. So the target truncates at
/// the first NUL (as the retired C harnesses did) and `text` applies the same
/// UTF-8 gate the bridge does.
pub struct Expr(Vec<u8>);

impl Expr {
    pub fn new(bytes: &[u8]) -> Option<Expr> {
        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let mut buf = Vec::new();
        // The crate's clippy.toml routes allocations through `falloc` so the
        // OOM sweep can fail them. The HARNESS must not be on that path: an
        // injected failure has to land in the code under test, not in the
        // scaffolding that feeds it. std's fallible reserve is the right call
        // here, and saying so is the point of the allow.
        #[allow(clippy::disallowed_methods)]
        buf.try_reserve_exact(len).ok()?;
        buf.extend_from_slice(&bytes[..len]);
        Some(Expr(buf))
    }

    pub fn text(&self) -> Option<VerifiedText<'_>> {
        VerifiedText::from_bytes(&self.0)
    }
}
