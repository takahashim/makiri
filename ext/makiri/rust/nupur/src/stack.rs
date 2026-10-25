//! The native-stack check an engine's recursion counters ask
//! (`ruby_stack_check`), handed out for a token.

#![allow(unsafe_code)]

use crate::rb::Ruby;

/// The check "is the current thread's (or Fiber's) Ruby-managed machine stack
/// close to exhausted?" (`ruby_stack_check`), as a plain function pointer for
/// an engine that knows nothing of Ruby to ask from its recursion counters.
///
/// Handed out only for a token: the function reads the current Ruby thread's
/// bookkeeping, so it is for code that runs where the token came from - a
/// registered method, with the GVL held - which the extension's engine calls
/// it from (none may in a GVL-released body).
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
    // SAFETY: `ruby_stack_check` takes no argument, raises nothing, and reads
    // only Ruby's own per-thread stack-bounds bookkeeping, which is valid
    // whenever Ruby is running at all.
    unsafe { rb_sys::ruby_stack_check() != 0 }
}
