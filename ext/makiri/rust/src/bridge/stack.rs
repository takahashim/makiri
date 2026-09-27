//! The one call to `rb_sys::ruby_stack_check`, installed into `crate::stack` at
//! `Init_makiri` so the engine's recursion checks can ask it without knowing
//! Ruby exists. See `crate::stack` for why a depth count alone is not enough.

#![allow(unsafe_code)]

/// Is the current thread's (or Fiber's) Ruby-managed machine stack close to
/// exhausted?
///
/// SAFETY: `ruby_stack_check` takes no argument, raises nothing, and reads
/// only Ruby's own per-thread stack-bounds bookkeeping, which is valid
/// whenever Ruby is running at all - true of every context this is callable
/// from, since it is reached only through a registered method (`entry`) or a
/// callback that already holds the GVL.
pub(crate) fn low() -> bool {
    unsafe { rb_sys::ruby_stack_check() != 0 }
}
