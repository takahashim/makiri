//! The native-stack check an engine's recursion counters ask
//! (`ruby_stack_check`), handed out for a token.

#![forbid(unsafe_code)]

use crate::rb::Ruby;

/// The check "is the current thread's (or Fiber's) Ruby-managed machine stack
/// close to exhausted?" (`ruby_stack_check`), as a plain function pointer for
/// an engine that knows nothing of Ruby to ask from its recursion counters.
///
/// Handed out only for a token. The function reads the calling thread's own
/// stack bounds, so it may be asked from a GVL-released body too (Makiri's
/// XML parse asks it from its DTD content-model check); a thread Ruby does
/// not run answers "room left".
pub fn stack_probe(_: &Ruby) -> fn() -> bool {
    stack_low
}

#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: handed out only by `stack_probe`, for a token"
    )
)]
fn stack_low() -> bool {
    /* The pointer outlives the token it was handed out for, so a call can
     * come from a thread Ruby does not run: `ruby_stack_low` answers "room
     * left" there. */
    nupur_sys::non_raising::ruby_stack_low()
}
