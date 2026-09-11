//! The C AST, viewed as Rust slices.
//!
//! `mkr_node_t` stores its arrays as a pointer plus a count, and a count of zero
//! leaves the pointer unset - so turning one into a slice is the same two lines
//! everywhere, and doing it once means no caller has to remember the empty case.

use super::abi::*;

/// A path's step list, empty when there are none.
///
/// # Safety
/// `steps` must name `n` live `mkr_step_t`, and the returned slice borrows the
/// AST node that owns them - so it must not outlive that node.
pub unsafe fn path_steps<'a>(steps: *mut Step, n: usize) -> &'a [Step] {
    if n == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(steps, n)
    }
}

/// A step's predicate list. Empty when there are none, so the pointer is never
/// read for a count of zero.
///
/// # Safety
/// Same as `path_steps`: the slice borrows the step's owning AST node.
pub unsafe fn step_preds<'a>(step: *const Step) -> &'a [*mut Node] {
    if (*step).npredicates == 0 {
        &[]
    } else {
        core::slice::from_raw_parts((*step).predicates, (*step).npredicates)
    }
}
