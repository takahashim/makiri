//! CSS selector front end: lowers a Lexbor-parsed selector
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
//! The lowering builds ordinary `xpath::ast` values through `build`: a node under
//! construction is a `build::Built`, and steps and lists are plain owned data, so
//! a failure anywhere drops - frees - what was built.

#![allow(clippy::missing_safety_doc)]

mod build;
mod lower;
mod parser;

use crate::xpath::ast::{Ast, Expr, Op};
use core::cell::RefCell;
use core::ffi::c_int;

use crate::falloc::try_box;
use crate::text::VerifiedText;
use crate::xpath::limits::Budget;
use crate::xpath::msg::{ErrSink, Reported};

/// The namespace context the glue hands in.
///
/// `default_namespace` says the document has a default namespace in scope, which
/// a bare type selector then binds to under the synthetic [`DEFAULT_NS_PREFIX`]
/// (Nokogiri's `"xmlns"` convention).
pub struct CssNs {
    pub default_namespace: bool,
}

/// The synthetic prefix bound to the document's default namespace.
pub const DEFAULT_NS_PREFIX: &[u8] = b"xmlns";

/// The cap on compounds in one selector chain - a selector-complexity bound.
pub const MAX_COMPOUNDS: usize = 64;

/// `MKR_XPATH_ERR_*`, as the C names them.
pub const ERR_SYNTAX: c_int = crate::xpath::msg::XP_ERR_SYNTAX;
pub const ERR_OOM: c_int = crate::xpath::msg::XP_ERR_OOM;
pub const ERR_LIMIT: c_int = crate::xpath::msg::XP_ERR_LIMIT;
pub const ERR_INTERNAL: c_int = crate::xpath::msg::XP_ERR_INTERNAL;

/// What every builder in this module carries: where to charge AST nodes, where
/// to report a failure, and the namespace context.
///
/// The builders take it shared - an operand is built in the argument list of the
/// node that takes it - so the budget they all charge sits in a `RefCell`.
pub(crate) struct Build<'a> {
    pub budget: RefCell<&'a mut Budget>,
    pub err: ErrSink,
    pub default_namespace: bool,
}

impl Build<'_> {
    pub(crate) fn fail(&self, status: c_int, msg: &core::ffi::CStr) -> Reported {
        crate::xpath::msg::err_set(self.err, status, msg)
    }

    pub(crate) fn oom(&self) -> Reported {
        self.fail(ERR_OOM, c"out of memory (css)")
    }

    /// The default-namespace prefix in scope, if any.
    pub(crate) fn default_prefix(&self) -> Option<&'static [u8]> {
        self.default_namespace.then_some(DEFAULT_NS_PREFIX)
    }
}

/// Compile `selector` into a freshly allocated AST.
///
/// `Err` with the budget's error slot filled: SYNTAX for a malformed selector or an
/// unsupported construct (jQuery extensions, pseudo-elements, the case
/// modifier), OOM or LIMIT for an allocation failure or the complexity cap.
///
/// # Safety
/// From the XPath/CSS glue, under the GVL.
pub unsafe fn compile_owned(
    selector: VerifiedText,
    ns: &CssNs,
    budget: &mut Budget,
) -> Result<Box<Ast>, Reported> {
    let err = budget.sink();
    let b = Build {
        budget: RefCell::new(budget),
        err,
        default_namespace: ns.default_namespace,
    };

    let parsed = match parser::parse(selector) {
        Ok(p) => p,
        Err(parser::ParseError::NotReady) => {
            return Err(b.fail(ERR_INTERNAL, c"failed to initialise CSS parser"));
        }
        Err(parser::ParseError::Syntax) => {
            return Err(b.fail(ERR_SYNTAX, c"invalid CSS selector"));
        }
    };

    /* Lower each comma-group to a PATH and union them. `parsed` cleans the
     * parser's arena when it drops, on every path out of this function - the C
     * spelled that out at each return instead. */
    let mut acc: Option<Expr> = None;
    let mut g = parsed.first;
    while !g.is_null() {
        /* Top level: the first compound is a descendant of the context node. */
        let path = lower::complex(&b, (*g).first, false)?;
        acc = Some(match acc {
            None => path,
            Some(lhs) => build::binop(&b, Op::Union, Ok(lhs), Ok(path))?,
        });
        g = (*g).next;
    }
    /* Lexbor rejects an empty selector list before it gets here; answering it
     * anyway keeps every failure reported. */
    let root = acc.ok_or_else(|| b.fail(ERR_SYNTAX, c"empty CSS selector"))?;
    /* No peephole or hoisting pass: the lowering emits no `//` pair to fuse and
     * no subtree worth remembering, so its AST is used as built. */
    try_box(Ast::new(root)).map_err(|_| b.oom())
}
