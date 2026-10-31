//! The native-stack-exhaustion probe, installed by `bridge` and consulted by
//! the engine's own recursion counters (XPath parse/eval, `Depth`).
//!
//! A recursion depth *count* only bounds the stack IF the caller's own frame
//! started with a full stack. It does not: Ruby's machine stack is as small as
//! 128 KiB in a `Fiber` (`RUBY_FIBER_MACHINE_STACK_SIZE`), so a selector or
//! expression well inside a generous count can still exhaust the ACTUAL stack.
//! When it does, Ruby's stack-overflow handling raises `SystemStackError` by
//! unwinding with `longjmp`, which jumps past every Rust frame between the
//! overflow and the nearest `rb_protect` - skipping `Drop`, so whatever that
//! frame borrowed (the process-global CSS parser, `GvlCell`'s busy flag) is
//! never released: a selector nested a few thousand deep wedged XML's CSS
//! that way while it ran on a CSS-to-XPath lowering that recursed.
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
//! `rb_sys::ruby_stack_check`, and cargo tests and the fuzz targets never install
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
/// probe is installed - the engine's Ruby-free builds (cargo test, the fuzz
/// targets) - so recursion there is bounded by depth counts alone, as it
/// always was.
pub fn exhausted() -> bool {
    PROBE.get().is_some_and(|probe| probe())
}

/// How many nesting levels a [`Depth`] goes between asks of the probe. A
/// recursion's stack grows with its nesting alone - a sibling's frames are
/// gone before the next one's - and Ruby's check (`ruby_stack_check`) reports
/// low while a reserve for C frames is still left, so a few levels between
/// asks fit in that reserve: eight levels of the XML writers' frames come to
/// about 2 KiB (a 128 KiB Fiber overflowed some 500 levels deep), and
/// `spec/native_stack_guard_spec.rb` holds the writers in such a Fiber.
/// Asking at every level cost `to_xml` and `canonicalize` 13-18%.
pub const STACK_CHECK_EVERY: u32 = 8;

/// A recursion's nesting level and its limit, passed down as an argument:
/// the one way a recursive function of the engine bounds itself.
///
/// [`enter`](Self::enter) refuses a level past the limit AND a stack running
/// low ([`exhausted`]), so a recursion written with a `Depth` cannot bound
/// its count and forget the stack - a count alone assumes the caller's stack
/// was full-sized, which a small `Fiber` or a deep caller is not, and an
/// overflow kills the process rather than raising. There is nothing to undo
/// on the way out: the level returns with the frame that took it.
///
/// (XPath's parse and evaluation count in their `Budget` instead, whose
/// `enter_recursion` asks the probe too.)
#[derive(Clone, Copy, Debug)]
pub struct Depth {
    level: u32,
    max: u32,
}

/// Why [`Depth::enter`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooDeep {
    /// The level reached the recursion's limit.
    Limit,
    /// The native stack ran low before it ([`exhausted`]).
    StackLow,
}

impl Depth {
    /// The top of a recursion that may nest `max` levels: `enter` succeeds
    /// at levels `0..max`.
    #[inline]
    pub const fn top(max: u32) -> Depth {
        Depth { level: 0, max }
    }

    /// This level, from 0 at the top.
    #[inline]
    pub const fn level(self) -> u32 {
        self.level
    }

    /// Enter this level - refused at the limit, or when the stack runs low
    /// (asked every [`STACK_CHECK_EVERY`] levels) - and answer the depth to
    /// pass one level down.
    #[inline]
    pub fn enter(self) -> Result<Depth, TooDeep> {
        if self.level >= self.max {
            return Err(TooDeep::Limit);
        }
        if self.level.is_multiple_of(STACK_CHECK_EVERY) && exhausted() {
            return Err(TooDeep::StackLow);
        }
        Ok(Depth {
            level: self.level + 1,
            max: self.max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_admits_levels_below_the_limit_and_refuses_the_limit() {
        let mut d = Depth::top(3);
        for want in 1..=3 {
            d = d.enter().unwrap();
            assert_eq!(d.level(), want);
        }
        assert_eq!(d.enter().unwrap_err(), TooDeep::Limit);
        /* With no probe installed (a Ruby-free build) the stack never runs low. */
        assert_eq!(Depth::top(0).enter().unwrap_err(), TooDeep::Limit);
    }
}
