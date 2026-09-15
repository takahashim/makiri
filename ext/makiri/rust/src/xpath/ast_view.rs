//! Borrowed views over the pointer/count arrays in the AST.
//!
//! The AST layout remains in [`super::ast`]. These helpers are kept separate
//! because they are the unsafe view boundary, not AST construction or
//! destruction.

use super::abi::*;

/// Borrow a path's steps. A zero count never reads the pointer.
///
/// # Safety
/// `steps` must point to `n` live steps owned by the AST.
pub unsafe fn path_steps<'a>(steps: *mut Step, n: usize) -> &'a [Step] {
    if n == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(steps, n)
    }
}

/// Borrow a step's predicate list. A zero count never reads the pointer.
///
/// # Safety
/// `step` must point to a live AST step.
pub unsafe fn step_preds<'a>(step: *const Step) -> &'a [*mut Node] {
    if (*step).npredicates == 0 {
        &[]
    } else {
        core::slice::from_raw_parts((*step).predicates, (*step).npredicates)
    }
}
