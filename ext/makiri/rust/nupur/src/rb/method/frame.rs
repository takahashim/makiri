//! What a method asks of the call it is running - whether it was given a
//! block or keywords - and the yield to that block. Each reads the current
//! control frame, so it answers for a method only from its own body.

#![allow(unsafe_code)]

use crate::rb::error::Error;
use crate::rb::ruby::Ruby;
use crate::rb::value::Value;

/// `rb_block_given_p`: whether the method running now was given a block.
/// Raises nothing and runs no Ruby code; like [`keyword_given`], it reads
/// the current frame.
#[inline]
pub fn block_given(ruby: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { nupur_sys::non_raising::rb_block_given_p(ruby.sys()) != 0 }
}

/// Yield `v` to the method's block (`rb_yield`): the caller's Ruby, which
/// can raise, `break`, `throw` or `return`. Each comes back as `Err` and is
/// passed on with `?`, so the trampoline resumes it as it left. Each call is
/// an `allow` of `runs_callers_ruby` at its site, saying why the block runs
/// there.
#[inline]
#[cfg_attr(
    dylint_lib = "nupur_lints",
    expect(
        runs_callers_ruby,
        reason = "the one way a method yields to its block; each caller's own allow says why"
    )
)]
pub fn yield_value(ruby: &Ruby, v: Value) -> Result<Value, Error> {
    crate::raw::yield_value(ruby, v)
}

/// [`yield_value`], and what the block's exit or exception came back as
/// DROPPED: a test suite's stand-in for a method that loses an `Err`, to
/// check that the trampoline still resumes the exit. Here, in the layer,
/// since an extension that forbids dropping an error cannot write it.
#[doc(hidden)]
pub fn yield_dropping_error_for_test(ruby: &Ruby) {
    crate::raw::yield_dropping_error_for_test(ruby)
}

/// `rb_keyword_given_p`: whether the method running now was called with
/// keywords - the one thing `rb_scan_args` asks Ruby when it separates them.
/// Raises nothing and runs no Ruby code. It reads the current frame, so it
/// answers for a method only from that method's own body, before the method
/// calls into Ruby (a method's argument scan is called first thing).
#[inline]
pub fn keyword_given(ruby: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { nupur_sys::non_raising::rb_keyword_given_p(ruby.sys()) != 0 }
}
