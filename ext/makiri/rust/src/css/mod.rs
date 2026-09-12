//! CSS selector front end (xpath/mkr_css.c): lowers a Lexbor-parsed selector
//! list into the native XPath engine's AST.
//!
//! No new evaluator opcodes. Every selector becomes existing PATH / step /
//! predicate nodes, so the shared evaluator's budgets, document order, dedup and
//! namespace resolution all apply unchanged.
//!
//! # This is a lowering, not a parser
//!
//! Lexbor parses the selector; this walks the result. The design doc lists
//! "re-implementing what Lexbor already provides - parsing, DOM, **selectors**,
//! encoding, serialization" among the things deliberately avoided, so replacing
//! the parser would be a separate decision with its own migration, not part of
//! moving a file between languages. See notes/rust_port_remaining.ja.md step 9.
//!
//! Lexbor's selector parser is used rather than its matcher for one further
//! reason: unlike the matcher, it preserves name case.
//!
//! # Ownership
//!
//! Every allocation matches `mkr_node_free`'s contract: nodes through
//! `mkr_node_alloc`, owned text through `mkr_owned_text_from_borrowed_copy`,
//! arrays through the C allocator. On any failure the builder returns `None`
//! with `*err` set and frees what it built, so a partial AST never escapes.
//!
//! Rust's `Drop` does not help here: these are raw C-owned nodes whose free
//! function is `mkr_node_free`, and wrapping them would mean either a newtype
//! per field or a `Drop` that duplicates the C's recursive free. The helpers
//! below take ownership explicitly, as the C's did, and every early return
//! frees on the way out.

#![allow(clippy::missing_safety_doc)]

mod build;
mod lower;
mod parser;

use core::ffi::{c_char, c_int};

use crate::xpath_abi::{
    mkr_err_set, mkr_node_free, Error, Limits, Node, VerifiedText, OP_UNION,
};

/// `mkr_css_ns_t` - the namespace context the glue hands in.
///
/// `default_prefix` is the synthetic prefix bound to the document's default
/// namespace (Nokogiri's `"xmlns"` convention) when one is in scope, else NULL.
#[repr(C)]
pub struct CssNs {
    pub default_prefix: *const c_char,
}

/// The synthetic prefix bound to the document's default namespace.
///
/// The C reads `default_prefix` as a pointer but takes its LENGTH from this
/// literal, because the glue only ever passes the sentinel. Keeping that here
/// preserves the property that no `strlen` runs on a pointer nothing verified.
pub const DEFAULT_NS_PREFIX: &[u8] = b"xmlns";

/// The cap on compounds in one selector chain - a selector-complexity bound.
pub const MAX_COMPOUNDS: usize = 64;

/// `MKR_XPATH_ERR_*`, as the C names them.
pub const ERR_SYNTAX: c_int = crate::xpath_abi::XP_ERR_SYNTAX;
pub const ERR_OOM: c_int = crate::xpath_abi::XP_ERR_OOM;
pub const ERR_LIMIT: c_int = crate::xpath_abi::XP_ERR_LIMIT;
pub const ERR_INTERNAL: c_int = crate::xpath_abi::XP_ERR_INTERNAL;

/// What every builder in this module carries: where to charge AST nodes, where
/// to report a failure, and the namespace context.
pub(crate) struct Build {
    pub limits: *mut Limits,
    pub err: *mut Error,
    pub ns: *const CssNs,
}

impl Build {
    pub(crate) unsafe fn fail(&self, status: c_int, msg: &core::ffi::CStr) {
        mkr_err_set(self.err, status, msg.as_ptr());
    }

    pub(crate) unsafe fn oom(&self) {
        self.fail(ERR_OOM, c"out of memory (css)");
    }

    /// The default-namespace prefix in scope, if any.
    pub(crate) unsafe fn default_prefix(&self) -> Option<&'static [u8]> {
        if self.ns.is_null() || (*self.ns).default_prefix.is_null() {
            return None;
        }
        Some(DEFAULT_NS_PREFIX)
    }
}

/// Compile `selector` into a freshly allocated AST, which the caller frees with
/// `mkr_node_free`.
///
/// NULL on error with `*err` filled: SYNTAX for a malformed selector or an
/// unsupported construct (jQuery extensions, pseudo-elements, the case
/// modifier), OOM or LIMIT for an allocation failure or the complexity cap.
/// `ns` may be NULL, in which case a bare selector matches no namespace.
///
/// # Safety
/// From the XPath/CSS glue, under the GVL.
#[no_mangle]
pub unsafe extern "C" fn mkr_css_compile(
    selector: VerifiedText,
    ns: *const CssNs,
    limits: *mut Limits,
    err: *mut Error,
) -> *mut Node {
    let b = Build { limits, err, ns };

    let parsed = match parser::parse(selector) {
        Ok(p) => p,
        Err(parser::ParseError::NotReady) => {
            b.fail(ERR_INTERNAL, c"failed to initialise CSS parser");
            return core::ptr::null_mut();
        }
        Err(parser::ParseError::Syntax) => {
            b.fail(ERR_SYNTAX, c"invalid CSS selector");
            return core::ptr::null_mut();
        }
    };

    /* Lower each comma-group to a PATH and union them. `parsed` cleans the
     * parser's arena when it drops, on every path out of this function - the C
     * spelled that out at each return instead. */
    let mut acc: *mut Node = core::ptr::null_mut();
    let mut g = parsed.first;
    while !g.is_null() {
        /* Top level: the first compound is a descendant of the context node. */
        let path = lower::complex(&b, (*g).first, false);
        if path.is_null() {
            mkr_node_free(acc);
            return core::ptr::null_mut();
        }
        acc = if acc.is_null() { path } else { build::binop(&b, OP_UNION, acc, path) };
        if acc.is_null() {
            /* binop freed both operands. */
            return core::ptr::null_mut();
        }
        g = (*g).next;
    }
    acc /* NULL with *err set on failure */
}
