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
//! the parser would be a separate decision with its own migration.
//!
//! Lexbor's selector parser is used rather than its matcher for one further
//! reason: unlike the matcher, it preserves name case.
//!
//! # Ownership
//!
//! The lowering builds ordinary `xpath::ast` values through `build`: a node under
//! construction is a `build::Built`, and steps and lists are plain owned data, so
//! a failure anywhere drops - frees - what was built.

#![forbid(unsafe_code)]

mod build;
mod lower;

use crate::lexbor::css_parser;
use crate::xpath::ast::{Ast, Op};
use core::cell::RefCell;
use core::ffi::c_int;

use crate::falloc::try_box;
use crate::text::VerifiedText;
use crate::xpath::limits::Budget;
use crate::xpath::msg::{ErrSink, Reported, XP_ERR_INTERNAL, XP_ERR_OOM, XP_ERR_SYNTAX};

/// The namespace context the glue hands in.
///
/// `default_namespace` says the document has a default namespace in scope, which
/// a bare type selector then binds to under the synthetic [`DEFAULT_NS_PREFIX`]
/// (Nokogiri's `"xmlns"` convention).
pub struct CssNs {
    pub default_namespace: bool,
}

/// The synthetic prefix bound to the document's default namespace. The glue
/// looks for it in the namespace hash the Ruby side normalises, and a bare type
/// selector is lowered under it, so both read this one constant.
pub const DEFAULT_NS_PREFIX: &str = "xmlns";

/// The cap on compounds in one selector chain - a selector-complexity bound.
pub(crate) const MAX_COMPOUNDS: usize = 64;

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
        crate::xpath::msg::err_set(self.err.clone(), status, msg)
    }

    pub(crate) fn oom(&self) -> Reported {
        self.fail(XP_ERR_OOM, c"out of memory (css)")
    }

    /// The default-namespace prefix in scope, if any.
    pub(crate) fn default_prefix(&self) -> Option<&'static [u8]> {
        self.default_namespace
            .then_some(DEFAULT_NS_PREFIX.as_bytes())
    }
}

/// Compile `selector` into a freshly allocated AST.
///
/// `Err` with the budget's error slot filled: SYNTAX for a malformed selector or an
/// unsupported construct (jQuery extensions, pseudo-elements, the case
/// modifier), OOM or LIMIT for an allocation failure or the complexity cap.
///
/// Safe: the selector is a [`VerifiedText`], and the only other condition -
/// running under the GVL - is the caller's by construction.
pub fn compile_owned(
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

    let parsed = match css_parser::parse(selector) {
        Ok(p) => p,
        Err(css_parser::ParseError::NotReady) => {
            return Err(b.fail(XP_ERR_INTERNAL, c"failed to initialise CSS parser"));
        }
        Err(css_parser::ParseError::Syntax) => {
            return Err(b.fail(XP_ERR_SYNTAX, c"invalid CSS selector"));
        }
    };

    /* Lower each comma-group to a PATH and union them. `parsed` cleans the
     * parser's arena when it drops, on every path out of this function. Lexbor
     * rejects an empty selector list before it gets here. */
    let root = build::fold(
        &b,
        Op::Union,
        parsed.groups().map(|g| {
            /* Top level: the first compound is a descendant of the context node. */
            lower::complex(&b, g.first(), false)
        }),
        c"empty CSS selector",
    )?;
    /* No peephole or hoisting pass: the lowering emits no `//` pair to fuse and
     * no subtree worth remembering, so its AST is used as built. */
    try_box(Ast::new(root)).map_err(|_| b.oom())
}
