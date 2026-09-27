//! The native-stack-exhaustion probe, installed by `bridge` and consulted by
//! the engine's own recursion counters (CSS lowering, XPath parse/eval).
//!
//! A recursion depth *count* only bounds the stack IF the caller's own frame
//! started with a full stack. It does not: Ruby's machine stack is as small as
//! 128 KiB in a `Fiber` (`RUBY_FIBER_MACHINE_STACK_SIZE`), so a selector or
//! expression well inside a generous count can still exhaust the ACTUAL stack.
//! When it does, Ruby's stack-overflow handling raises `SystemStackError` by
//! unwinding with `longjmp`, which jumps past every Rust frame between the
//! overflow and the nearest `rb_protect` - skipping `Drop`, so whatever that
//! frame borrowed (the process-global CSS parser, `GvlCell`'s busy flag) is
//! never released. See `css::MAX_SELECTOR_NESTING` for the incident this
//! fixes.
//!
//! `ruby_stack_check()` answers the real question - is there room left in
//! THIS execution context, Fiber included - so checking it at every recursion
//! step turns the failure into an ordinary `Limit` raise (a ordinary return,
//! no `longjmp`) before the native stack actually runs out, whatever the
//! caller's stack size, instead of one that skips `Drop`.
//!
//! This module holds only the seam: a `fn() -> bool` installed once at
//! `Init_makiri` and read by every recursion check. It is `#![forbid(unsafe_code)]`
//! and Ruby-free itself; `bridge::stack` is the one place that calls
//! `rb_sys::ruby_stack_check`, and cargo tests / Kani / fuzz never install
//! anything, so `exhausted()` is a constant `false` there - the engine never
//! depends on Ruby to build or run standalone.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

static PROBE: OnceLock<fn() -> bool> = OnceLock::new();

/// Install the probe. Only `Init_makiri` should call this, once, before any
/// recursive engine call can run; a second call is a no-op (`OnceLock::set`
/// leaves the first winner in place), which is fine since there is only ever
/// one probe worth installing in a process.
pub fn install(probe: fn() -> bool) {
    let _ = PROBE.set(probe);
}

/// Is the current execution context's native stack low? `false` whenever no
/// probe is installed - the engine's Ruby-free builds (cargo test, Kani, the
/// fuzz targets) - so recursion there is bounded by depth counts alone, as it
/// always was.
pub fn exhausted() -> bool {
    PROBE.get().is_some_and(|probe| probe())
}
