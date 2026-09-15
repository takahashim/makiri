//! Shared entry points and helpers for the targets.
//!
//! The targets call the same crate-level functions the Ruby glue calls - the
//! XML reader, the XPath context, parse and evaluate. There is no C boundary to
//! go through: the extension exports nothing but `Init_makiri`. What the glue
//! relies on still holds here, and is what the targets exercise: raw context and
//! AST handles whose ownership crosses each call, and out-parameter statuses.

// Each target is its own crate and uses only part of this module.
#![allow(dead_code, unused_imports)]

use core::ffi::{c_int, c_void};

pub use makiri::text::VerifiedText;
pub use makiri::xml::parse::mkr_xml_parse;
pub use makiri::xml::Document;
pub use makiri::xpath::ast_ops::mkr_node_free;
pub use makiri::xpath::boundary::{mkr_xpath_error_clear, mkr_xpath_value_clear};
pub use makiri::xpath::ctx::{
    mkr_ctx_limits, mkr_xpath_context_free, mkr_xpath_context_new, mkr_xpath_register_ns,
    mkr_xpath_set_engine_kind,
};
pub use makiri::xpath::evaluate::mkr_xpath_eval_compiled;
pub use makiri::xpath::limits::mkr_xpath_limits_init_defaults;
pub use makiri::xpath::parse::mkr_parse;
pub use makiri::xpath_abi::{Context, Error as XPathError, Limits, XPathValue};

/// The engine kind the XML monomorphization answers to. Every target pins it.
pub const ENGINE_XML: c_int = 1;

/// A context over `doc`, rooted at its document node and pinned to the XML
/// engine - the same arguments the glue's `build_ctx` passes. `None` when the
/// document has no root or the context cannot be allocated. The caller frees
/// the context before `doc` drops.
///
/// # Safety
/// `doc` must outlive the returned context.
pub unsafe fn xml_context(doc: &mut Document) -> Option<*mut Context> {
    if doc.doc_node.is_invalid() {
        return None;
    }
    let node = doc.doc_node.to_token() as *mut c_void;
    let ctx = mkr_xpath_context_new(doc as *mut Document as *mut c_void, node);
    if ctx.is_null() {
        return None;
    }
    mkr_xpath_set_engine_kind(ctx, ENGINE_XML);
    Some(ctx)
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
