//! The native-stack check an engine's recursion counters ask
//! (`ruby_stack_check`), handed out for a token.

#![allow(unsafe_code)]

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
     * come from a thread Ruby does not run, which has no execution context to
     * read: it answers "room left", as a build with no probe does. A Ruby
     * thread - with the GVL or without it - reads its own stack's bounds. */
    // SAFETY: `ruby_native_thread_p` reads only the calling thread's own
    // thread-local state, from any thread; `ruby_stack_check` takes no
    // argument, raises nothing, and reads that Ruby thread's stack bounds.
    unsafe { rb_sys::ruby_native_thread_p() != 0 && rb_sys::ruby_stack_check() != 0 }
}
