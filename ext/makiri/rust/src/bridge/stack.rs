//! The Ruby stack check (`nupur::helpers::stack_low`), installed into
//! `crate::stack` at `Init_makiri` so the engine's recursion checks can ask it
//! without knowing Ruby exists. See `crate::stack` for why a depth count alone
//! is not enough.

#![forbid(unsafe_code)]

/// Is the current thread's (or Fiber's) Ruby-managed machine stack close to
/// exhausted?
pub(crate) fn low() -> bool {
    crate::nupur::helpers::stack_low()
}
