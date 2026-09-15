//! The per-evaluate budgets (mkr_xpath.c's limits section), and the error slot a
//! run reports into, owned together as a [`Budget`].
//!
//! Every overrun is XP_ERR_LIMIT - never a truncated or empty result. The
//! counters live in the context, and the glue reads the struct directly (it
//! resets `ast_nodes` before each parse), so its fields are public.

/* Each function here takes the `*mut Budget` the caller already holds, so the
 * contract is the pointer's, stated once. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use crate::err_setf;

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
    pub ast_nodes: usize,
    pub eval_ops: usize,
    pub recursion_depth: usize,
}

/* The two counters below are charged once per visited node, so they are the
 * hottest functions in the engine. Two things have to stay out of them.
 *
 * The message. `err_setf!` assembles into a 200-byte stack buffer, and a frame
 * that large in the caller costs more than the check it guards - so each
 * overrun reporter is its own #[cold] function and the hot path keeps a small
 * frame. (The C got this for free: its reporter was a call into another
 * translation unit.)
 *
 * The increment. `overflow-checks` is deliberately on in release (a wrapped
 * size is exactly the class of bug fail-closed exists to stop), so `+= 1`
 * carries a panic branch. Comparing before incrementing makes the counter
 * provably below its cap at the add, and keeps the budget identical: the C
 * increments then rejects `> max`, which admits exactly `max` ops, and so does
 * this. */

/// A run's budgets and the slot its failure is written to: what every charged
/// or reported step needs, owned together by the context, the parser or the CSS
/// lowering doing the run.
pub struct Budget {
    pub limits: Limits,
    pub err: Error,
}

impl Budget {
    /// Default budgets, and no error yet.
    pub fn new() -> Budget {
        let mut b = Budget {
            // SAFETY: every field is an integer; init_defaults sets them all.
            limits: unsafe { core::mem::zeroed() },
            err: Error::new(),
        };
        unsafe { xpath_limits_init_defaults(&mut b.limits) };
        b
    }

    /// The failure written so far, leaving the slot empty for the next run.
    pub fn take_error(&mut self) -> Error {
        core::mem::take(&mut self.err)
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new()
    }
}

/// Where `b` reports: its error slot, or nowhere for a null budget - which is
/// also unbounded, the pairing the best-effort paths want.
#[inline]
pub unsafe fn budget_sink(b: *mut Budget) -> ErrSink {
    if b.is_null() {
        ErrSink::silent()
    } else {
        ErrSink::new(&mut (*b).err)
    }
}

#[cold]
#[inline(never)]
unsafe fn over_ast_nodes(l: *mut Limits, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "AST node limit exceeded ({})",
        (*l).max_ast_nodes
    )
}

#[cold]
#[inline(never)]
unsafe fn over_eval_ops(l: *mut Limits, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "evaluation budget exceeded ({} ops)",
        (*l).max_eval_ops
    )
}

#[cold]
#[inline(never)]
unsafe fn over_recursion(l: *mut Limits, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "recursion depth limit exceeded ({})",
        (*l).max_recursion_depth
    )
}

#[cold]
#[inline(never)]
unsafe fn over_check(max: usize, noun: &str, err: ErrSink) -> Reported {
    err_setf!(err, XP_ERR_LIMIT, "{} limit exceeded ({})", noun, max)
}

#[cold]
#[inline(never)]
unsafe fn over_string_bytes(max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "string size limit exceeded ({} bytes)",
        max
    )
}

#[cold]
#[inline(never)]
unsafe fn over_expr_bytes(bytes: usize, max: usize, err: ErrSink) -> Reported {
    err_setf!(
        err,
        XP_ERR_LIMIT,
        "expression too long ({} bytes, max {})",
        bytes,
        max
    )
}

/// Defaults aimed safely above realistic queries.
///
/// # Safety
/// `l` must point to a writable `mkr_xpath_limits_t`.
pub unsafe fn xpath_limits_init_defaults(l: *mut Limits) {
    *l = Limits {
        max_expr_bytes: 64 * 1024, /* 64 KB XPath string */
        max_ast_nodes: 100_000,
        max_steps: 256,     /* path step count */
        max_predicates: 64, /* per-step predicates */
        max_function_args: 64,
        max_nodeset_size: 10 * 1000 * 1000, /* 10M nodes - large but bounded */
        max_eval_ops: 50 * 1000 * 1000,     /* 50M evaluator steps */
        max_string_bytes: 64 * 1024 * 1024, /* 64 MB string-value */
        max_recursion_depth: 256,
        ast_nodes: 0,
        eval_ops: 0,
        recursion_depth: 0,
    };
}

pub unsafe fn limit_ast_node(b: *mut Budget) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    if (*l).ast_nodes >= (*l).max_ast_nodes {
        return Err(over_ast_nodes(l, err));
    }
    (*l).ast_nodes += 1;
    Ok(())
}

/// THE evaluator progress gate.
///
/// This is the single primitive that bounds runtime work: every loop in the
/// engine whose trip count is input-derived charges ONE tick per iteration
/// through here - the axis walk per visited node, the M*N compare per pair, the
/// index-bucket scan per element, eval_node per AST node. One uniform rule, so
/// checking the DoS bound is local: confirm each such loop calls this.
///
/// Kept deliberately uniform, with no bulk variant: a bulk charge would only
/// suit run-to-completion loops and would wrongly reject an early-exiting query
/// if misapplied, trading one foot-gun-free rule for a conditional one.
pub unsafe fn limit_eval_op(b: *mut Budget) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    if (*l).eval_ops >= (*l).max_eval_ops {
        return Err(over_eval_ops(l, err));
    }
    (*l).eval_ops += 1;
    Ok(())
}

pub unsafe fn limit_recurse_enter(b: *mut Budget) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    if (*l).recursion_depth >= (*l).max_recursion_depth {
        /* The C increments, reports, then backs the failed entry out; comparing
         * first never counts it in the first place. */
        return Err(over_recursion(l, err));
    }
    (*l).recursion_depth += 1;
    Ok(())
}

pub unsafe fn limit_recurse_leave(b: *mut Budget) {
    let l = &raw mut (*b).limits;
    if (*l).recursion_depth > 0 {
        (*l).recursion_depth -= 1;
    }
}

/// The shared "value must not exceed max" gate for the count checks. The
/// byte-oriented ones keep their own wording.
unsafe fn check(value: usize, max: usize, noun: &str, err: ErrSink) -> Result<(), Reported> {
    if value > max {
        return Err(over_check(max, noun, err));
    }
    Ok(())
}

pub unsafe fn limit_check_nodeset_size(b: *mut Budget, new_count: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    check(new_count, (*l).max_nodeset_size, "nodeset size", err)
}

pub unsafe fn limit_check_string_bytes(b: *mut Budget, bytes: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    if bytes > (*l).max_string_bytes {
        return Err(over_string_bytes((*l).max_string_bytes, err));
    }
    Ok(())
}

pub unsafe fn limit_check_steps(b: *mut Budget, nsteps: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    check(nsteps, (*l).max_steps, "path step count", err)
}

pub unsafe fn limit_check_predicates(b: *mut Budget, npreds: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    check(npreds, (*l).max_predicates, "predicate count", err)
}

pub unsafe fn limit_check_func_args(b: *mut Budget, nargs: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    check(
        nargs,
        (*l).max_function_args,
        "function argument count",
        err,
    )
}

pub unsafe fn limit_check_expr_bytes(b: *mut Budget, bytes: usize) -> Result<(), Reported> {
    let (l, err) = (&raw mut (*b).limits, budget_sink(b));
    if bytes > (*l).max_expr_bytes {
        return Err(over_expr_bytes(bytes, (*l).max_expr_bytes, err));
    }
    Ok(())
}
