//! Shared entry points and helpers for the targets.
//!
//! The targets go through the engine's front door, the one the Ruby glue uses:
//! an [`OwnedContext`] over a document, [`parse_owned`] against the context's
//! [`Budget`], and [`evaluate`] / [`evaluate_first`] returning an owned
//! [`XPathValue`]. Every handle is owned, so an early return frees it.

// Each target is its own crate and uses only part of this module.
#![allow(dead_code, unused_imports)]

use core::ffi::c_void;

pub use makiri::text::VerifiedText;
pub use makiri::xml::parse::mkr_xml_parse;
pub use makiri::xml::Document;
pub use makiri::xpath::ctx::{
    evaluate, evaluate_first, xpath_register_ns, Backend, OwnedContext, XPathValue,
};
pub use makiri::xpath::limits::Budget;
pub use makiri::xpath::own::Ast;
pub use makiri::xpath::parse::parse_owned;
pub use makiri::xpath::ctx::{ctx_budget, Context};
pub use makiri::xpath::limits::Limits;

/// A context over `doc`, rooted at its document node and pinned to the XML
/// engine - the same arguments the glue's `build_ctx` passes. `None` when the
/// document has no root or the context cannot be allocated.
///
/// # Safety
/// `doc` must outlive the returned context.
pub unsafe fn xml_context(doc: &mut Document) -> Option<OwnedContext> {
    if doc.doc_node.is_invalid() {
        return None;
    }
    let node = doc.doc_node.to_token() as *mut c_void;
    OwnedContext::new(doc as *mut Document as *mut c_void, node, Backend::Xml)
}

/// The context's budgets, for a target to tighten before it parses or
/// evaluates.
///
/// # Safety
/// `ctx` must be live, and the borrow must end before the next engine call on
/// it.
pub unsafe fn limits<'a>(ctx: *mut Context) -> &'a mut Limits {
    &mut (*ctx_budget(ctx)).limits
}

/// Parse `text` against the context's budget, with the AST counter reset the
/// way the glue resets it before each parse.
///
/// # Safety
/// `ctx` must be live.
pub unsafe fn parse(ctx: &OwnedContext, text: VerifiedText) -> Option<Ast> {
    let budget = ctx_budget(ctx.as_ptr());
    (*budget).limits.ast_nodes = 0;
    parse_owned(text, budget).ok()
}

/// Evaluate `ast` both ways the glue does, and hold the `at_xpath` fast path to
/// its promise: when both succeed with a node-set, `evaluate_first` answers the
/// first node of what `evaluate` answers. A failure on either side is not
/// compared - the fast path is allowed to finish a walk the full evaluator
/// overruns.
///
/// # Safety
/// `ctx` must be live and `ast` parsed for it.
pub unsafe fn evaluate_both(ctx: &OwnedContext, ast: &Ast) {
    let full = evaluate(ctx.as_ptr(), ast.as_raw());
    let first = evaluate_first(ctx.as_ptr(), ast.as_raw());
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

    pub fn text(&self) -> Option<VerifiedText> {
        VerifiedText::from_bytes(&self.0)
    }
}
