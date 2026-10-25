//! The Ruby stack check (`nupur::stack::stack_probe`), installed into
//! `crate::stack` at `Init_makiri` so the engine's recursion checks can ask it
//! without knowing Ruby exists. See `crate::stack` for why a depth count alone
//! is not enough.

#![forbid(unsafe_code)]

/// The probe "is the current thread's (or Fiber's) Ruby-managed machine stack
/// close to exhausted?", for `crate::stack::install`.
pub(crate) fn probe(ruby: &crate::bridge::rb::Ruby) -> fn() -> bool {
    crate::nupur::stack::stack_probe(ruby)
}
