//! The per-run budgets: the caps a context is configured with ([`Limits`]),
//! and what one run charges against them and reports into ([`Budget`]).
//!
//! Every overrun is XP_ERR_LIMIT - never a truncated or empty result.

#![forbid(unsafe_code)]

use super::abi::*;
use crate::err_setf;
use core::cell::{Cell, RefCell};
use std::rc::Rc;

/// The caps, as configured. Plain data: a run copies them into its [`Budget`].
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_expr_bytes: usize,
    pub max_ast_nodes: usize,
    pub max_steps: usize,
    pub max_predicates: usize,
    pub max_function_args: usize,
    pub max_nodeset_size: usize,
    pub max_eval_ops: usize,
    pub max_string_bytes: usize,
    pub max_recursion_depth: usize,
}

impl Limits {
    /// Defaults aimed safely above realistic queries.
    pub const DEFAULT: Limits = Limits {
        max_expr_bytes: 64 * 1024, /* 64 KB XPath string */
        max_ast_nodes: 100_000,
        max_steps: 256,     /* path step count */
        max_predicates: 64, /* per-step predicates */
        max_function_args: 64,
        max_nodeset_size: 10 * 1000 * 1000, /* 10M nodes - large but bounded */
        max_eval_ops: 50 * 1000 * 1000,     /* 50M evaluator steps */
        max_string_bytes: 64 * 1024 * 1024, /* 64 MB string-value */
        max_recursion_depth: 256,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

/// A run's caps, what it has charged against them, and the slot its failure is
/// written to - owned together by the context, the parser or the CSS lowering
/// doing the run.
pub struct Budget {
    pub limits: Limits,
    /// AST nodes built by the current parse.
    ast_nodes: Cell<usize>,
    /// Evaluator steps charged by the current evaluate.
    eval_ops: Cell<usize>,
    /// The current evaluation (or parse) recursion depth.
    recursion_depth: Cell<usize>,
    /// The failure slot. A shared `RefCell` rather than a plain field so a sink
    /// is a safe, copyable handle to it: the CSS lowering holds its budget and
    /// its sink at once, and a function keeps its sink across calls that take
    /// `&mut` of the rest of the run.
    err: Rc<RefCell<Error>>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new()
    }
}

/* The two counters charged once per visited node - `charge_op` and the
 * recursion pair - are the hottest functions in the engine. Two things have to
 * stay out of them.
 *
 * The message. `err_setf!` assembles into a 200-byte stack buffer, and a frame
 * that large in the caller costs more than the check it guards - so each
 * overrun reporter is its own #[cold] function and the hot path keeps a small
 * frame.
 *
 * The increment. `overflow-checks` is deliberately on in release (a wrapped
 * size is exactly the class of bug fail-closed exists to stop), so `+= 1`
 * carries a panic branch. Comparing before incrementing makes the counter
 * provably below its cap at the add, and admits exactly `max` charges. */
impl Budget {
    /// Default caps, nothing charged, and no error yet.
    pub fn new() -> Budget {
        Budget::with_limits(Limits::DEFAULT)
    }

    /// `limits`, nothing charged, and no error yet.
    pub fn with_limits(limits: Limits) -> Budget {
        Budget {
            limits,
            ast_nodes: Cell::new(0),
            eval_ops: Cell::new(0),
            recursion_depth: Cell::new(0),
            err: Rc::new(RefCell::new(Error::new())),
        }
    }

    /// The failure written so far, leaving the slot empty for the next run.
    pub fn take_error(&self) -> Error {
        core::mem::take(&mut *self.err.borrow_mut())
    }

    /// Where this run reports.
    #[inline]
    pub fn sink(&self) -> ErrSink {
        ErrSink::new(Rc::clone(&self.err))
    }

    /// Charge one AST node.
    #[inline]
    pub fn charge_ast_node(&self) -> Result<(), Reported> {
        if self.ast_nodes.get() >= self.limits.max_ast_nodes {
            return Err(over_ast_nodes(self.limits.max_ast_nodes, self.sink()));
        }
        self.ast_nodes.set(self.ast_nodes.get() + 1);
        Ok(())
    }

    /// THE evaluator progress gate.
    ///
    /// This is the single primitive that bounds runtime work: every loop in the
    /// engine whose trip count is input-derived charges ONE tick per iteration
    /// through here - the axis walk per visited node, the M*N compare per pair,
    /// the index-bucket scan per element, eval_node per AST node. One uniform
    /// rule, so checking the DoS bound is local: confirm each such loop calls
    /// this.
    ///
    /// Kept deliberately uniform, with no bulk variant: a bulk charge would only
    /// suit run-to-completion loops and would wrongly reject an early-exiting
    /// query if misapplied, trading one foot-gun-free rule for a conditional one.
    #[inline]
    pub fn charge_op(&self) -> Result<(), Reported> {
        if self.eval_ops.get() >= self.limits.max_eval_ops {
            return Err(over_eval_ops(self.limits.max_eval_ops, self.sink()));
        }
        self.eval_ops.set(self.eval_ops.get() + 1);
        Ok(())
    }

    /// Enter one recursion level; a refused entry is not counted, so it needs no
    /// matching [`leave_recursion`](Self::leave_recursion).
    #[inline]
    pub fn enter_recursion(&self) -> Result<(), Reported> {
        if self.recursion_depth.get() >= self.limits.max_recursion_depth {
            return Err(over_recursion(self.limits.max_recursion_depth, self.sink()));
        }
        self.recursion_depth.set(self.recursion_depth.get() + 1);
        Ok(())
    }

    #[inline]
    pub fn leave_recursion(&self) {
        self.recursion_depth
            .set(self.recursion_depth.get().saturating_sub(1));
    }

    pub fn check_nodeset_size(&self, new_count: usize) -> Result<(), Reported> {
        check(
            new_count,
            self.limits.max_nodeset_size,
            "nodeset size",
            self.sink(),
        )
    }

    pub fn check_string_bytes(&self, bytes: usize) -> Result<(), Reported> {
        if bytes > self.limits.max_string_bytes {
            return Err(over_string_bytes(self.limits.max_string_bytes, self.sink()));
        }
        Ok(())
    }

    pub fn check_steps(&self, nsteps: usize) -> Result<(), Reported> {
        check(
            nsteps,
            self.limits.max_steps,
            "path step count",
            self.sink(),
        )
    }

    pub fn check_predicates(&self, npreds: usize) -> Result<(), Reported> {
        check(
            npreds,
            self.limits.max_predicates,
            "predicate count",
            self.sink(),
        )
    }

    pub fn check_func_args(&self, nargs: usize) -> Result<(), Reported> {
        check(
            nargs,
            self.limits.max_function_args,
            "function argument count",
            self.sink(),
        )
    }

    pub fn check_expr_bytes(&self, bytes: usize) -> Result<(), Reported> {
        if bytes > self.limits.max_expr_bytes {
            return Err(over_expr_bytes(
                bytes,
                self.limits.max_expr_bytes,
                self.sink(),
            ));
        }
        Ok(())
    }
}

#[cold]
#[inline(never)]
fn over_ast_nodes(max: usize, err: ErrSink) -> Reported {
    err_setf!(err, XP_ERR_LIMIT, "AST node limit exceeded ({})", max)
}

#[cold]
#[inline(never)]
fn over_eval_ops(max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "evaluation budget exceeded ({} ops)",
        max
    )
}

#[cold]
#[inline(never)]
fn over_recursion(max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "recursion depth limit exceeded ({})",
        max
    )
}

/// The deepest AST the parser and the CSS lowering build.
///
/// Every pass over an AST, its `Drop` included, recurses once per level, so the
/// depth is what decides how much native stack an expression can make them use.
/// Parser recursion is already bounded, but a chain of binary operators is read
/// in a loop, so without this `1+1+...` within the expression-length cap built a
/// tree tens of thousands of levels deep - enough to overflow a thread's stack.
///
/// It is not the evaluation limit: `max_recursion_depth` (256 by default) still
/// decides what evaluates, and a tree deeper than that fails there as before.
/// This is a fixed, generous bound on what may be built at all.
pub const MAX_AST_DEPTH: u32 = 1024;

/// Refuse a node that would make the AST deeper than [`MAX_AST_DEPTH`]. The
/// builders check each node as they make it, so a tree over the bound is never
/// finished - and never has to be taken apart at that depth.
pub fn check_ast_depth(e: &Expr, err: ErrSink) -> Result<(), Reported> {
    if e.depth() <= MAX_AST_DEPTH {
        Ok(())
    } else {
        Err(over_ast_depth(err))
    }
}

#[cold]
#[inline(never)]
fn over_ast_depth(err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "expression nesting depth limit exceeded ({})",
        MAX_AST_DEPTH
    )
}

#[cold]
#[inline(never)]
fn over_check(max: usize, noun: &str, err: ErrSink) -> Reported {
    err_setf!(err, XP_ERR_LIMIT, "{} limit exceeded ({})", noun, max)
}

#[cold]
#[inline(never)]
fn over_string_bytes(max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "string size limit exceeded ({} bytes)",
        max
    )
}

#[cold]
#[inline(never)]
fn over_expr_bytes(bytes: usize, max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "expression too long ({} bytes, max {})",
        bytes,
        max
    )
}

/// The shared "value must not exceed max" gate for the count checks. The
/// byte-oriented ones keep their own wording.
#[inline]
fn check(value: usize, max: usize, noun: &str, err: ErrSink) -> Result<(), Reported> {
    if value > max {
        return Err(over_check(max, noun, err));
    }
    Ok(())
}
