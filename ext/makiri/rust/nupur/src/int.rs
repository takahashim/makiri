//! Every conversion between Rust's integer widths and C's: the one module
//! that names a C `long` or casts with `as` at the Ruby boundary
//! (`rake dylint`'s `c_integer_outside_int` holds every other one to that).
//!
//! A C `long` is 64 bits on Linux and macOS and 32 bits on Windows (LLP64),
//! and Ruby's integers follow it: a Fixnum is `LONG_MIN / 2 ..= LONG_MAX /
//! 2`, so 2^30 - 1 at most on Windows. `c_long` is an alias, `i64` itself
//! where the code is built and linted, so a truncation there is invisible to
//! rustc and clippy - which is why each Windows bug was found by review. So
//! the rules are written once, over a [`Long`] of either width, and
//! instantiated at this target's: the tests run them at both widths on any
//! machine, and hold the host's instance to rb-sys's own constants and
//! `long2fix`.
//!
//! Everything else converts with `From` / `TryFrom`, which compile to what
//! each target allows and fail where a value does not fit, and never names
//! `c_long`: a length is a `usize`, an index or a count an `i64`.

#![forbid(unsafe_code)]
/* The one module that casts: each cast below is a widening a `const` assert
 * proves, or a test's. */
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the module whose casts are proved or tested; see each"
)]

use core::ffi::c_long;

/// A C `long` of some width: this target's (`c_long`), or the other one, for
/// the tests.
pub(crate) trait Long: Copy + Into<i64> + TryFrom<i64> + TryFrom<u64> {
    const MIN: Self;
    const MAX: Self;
}

impl Long for i32 {
    const MIN: i32 = i32::MIN;
    const MAX: i32 = i32::MAX;
}

impl Long for i64 {
    const MIN: i64 = i64::MIN;
    const MAX: i64 = i64::MAX;
}

/// `n` as a `long` within a Fixnum's range (`RB_FIXABLE`), or `None`: a
/// Fixnum is a `long` less its flag bit, `MIN / 2 ..= MAX / 2`.
#[inline]
pub(crate) fn fixable_in<L: Long>(n: i64) -> Option<L> {
    let (min, max) = (L::MIN.into() / 2, L::MAX.into() / 2);
    if n < min || n > max {
        return None;
    }
    L::try_from(n).ok()
}

/// [`fixable_in`] for an unsigned value (`RB_POSFIXABLE`).
#[inline]
pub(crate) fn posfixable_in<L: Long>(n: u64) -> Option<L> {
    i64::try_from(n).ok().and_then(fixable_in::<L>)
}

/// `n` as this target's `long`, when it is a Fixnum's.
#[inline]
pub(crate) fn fixable(n: i64) -> Option<c_long> {
    fixable_in::<c_long>(n)
}

/// `n` as this target's `long`, when it is a Fixnum's.
#[inline]
pub(crate) fn posfixable(n: u64) -> Option<c_long> {
    posfixable_in::<c_long>(n)
}

/// The Fixnum `n` is, as Ruby's `LONG2FIX` makes it - `(n << 1) | 1`, the
/// `long` sign-extended to a `VALUE` - or `None` past a Fixnum's range
/// (past 2^30 - 1 on Windows). Built from its bits here, where the tests
/// hold it to rb-sys's `long2fix`, so a caller needs no proof of the GVL for
/// what is arithmetic.
#[inline]
pub(crate) fn fixnum(n: i64) -> Option<rb_sys::VALUE> {
    fixable(n).map(fixnum_bits)
}

/// [`fixnum`] for an unsigned value.
#[inline]
pub(crate) fn posfixnum(n: u64) -> Option<rb_sys::VALUE> {
    posfixable(n).map(fixnum_bits)
}

#[inline]
fn fixnum_bits(l: c_long) -> rb_sys::VALUE {
    ((l as rb_sys::VALUE) << 1) | rb_sys::FIXNUM_FLAG as rb_sys::VALUE
}

/// A non-negative `long` the C API answered as a length (`RSTRING_LEN`,
/// `RARRAY_LEN`): a `usize`, and 0 for a negative one, which no length is.
#[inline]
pub(crate) fn len(n: c_long) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// A capacity for the C API: `n`, or the largest `long` for one past it - a
/// hint, which the allocation then refuses.
#[inline]
pub(crate) fn capacity(n: usize) -> c_long {
    c_long::try_from(n).unwrap_or(c_long::MAX)
}

/// `rb_sys::posfixable`'s argument, for the test that holds [`posfixable`]
/// to it.
#[cfg(test)]
fn as_ulong(n: u64) -> Option<core::ffi::c_ulong> {
    core::ffi::c_ulong::try_from(n).ok()
}

/// A word Ruby hands out (a `VALUE`) as a `usize`: the two are the same
/// width on every target, a `VALUE` being C's `uintptr_t`.
#[inline]
pub(crate) fn word(v: rb_sys::VALUE) -> usize {
    const _: () = assert!(size_of::<rb_sys::VALUE>() == size_of::<usize>());
    v as usize
}

/// A `usize` as a `u64`, which holds every one on every target Rust has.
#[inline]
pub(crate) fn wide(n: usize) -> u64 {
    const _: () = assert!(usize::BITS <= u64::BITS);
    n as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_sys::stable_api::{get_default, StableApiDefinition};

    /// A Windows `long`: 2^30 - 1 is the largest Fixnum, and a 64-bit key
    /// with anything in its high half is far past it.
    #[test]
    fn a_32_bit_long_has_31_bit_fixnums() {
        assert_eq!(fixable_in::<i32>((1 << 30) - 1), Some((1 << 30) - 1));
        assert_eq!(fixable_in::<i32>(1 << 30), None);
        assert_eq!(fixable_in::<i32>(-(1 << 30)), Some(-(1 << 30)));
        assert_eq!(fixable_in::<i32>(-(1 << 30) - 1), None);
        assert_eq!(fixable_in::<i32>(1 | (1 << 32)), None);
        assert_eq!(posfixable_in::<i32>(u64::MAX), None);
        assert_eq!(posfixable_in::<i32>(7), Some(7));
    }

    #[test]
    fn a_64_bit_long_has_63_bit_fixnums() {
        assert_eq!(fixable_in::<i64>((1 << 62) - 1), Some((1 << 62) - 1));
        assert_eq!(fixable_in::<i64>(1 << 62), None);
        assert_eq!(fixable_in::<i64>(-(1 << 62)), Some(-(1 << 62)));
        assert_eq!(fixable_in::<i64>(-(1 << 62) - 1), None);
        assert_eq!(posfixable_in::<i64>(1 << 62), None);
    }

    /// This target's instance is rb-sys's: the same bounds as its
    /// `FIXNUM_MIN` / `FIXNUM_MAX`, and the same answer as its `fixable`.
    #[test]
    fn this_targets_long_agrees_with_rb_sys() {
        use rb_sys::special_consts::{FIXNUM_MAX, FIXNUM_MIN};
        let api = get_default();
        let (min, max) = (i64::from(FIXNUM_MIN), i64::from(FIXNUM_MAX));
        for n in [0, 1, -1, max, max + 1, min, min - 1, i64::MAX, i64::MIN] {
            let ours = fixable(n);
            let theirs = c_long::try_from(n).ok().filter(|&l| api.fixable(l));
            assert_eq!(ours, theirs, "{n}");
        }
        for n in [0, 1, max as u64, max as u64 + 1, u64::MAX] {
            let ours = posfixable(n).is_some();
            let theirs = as_ulong(n).is_some_and(|l| api.posfixable(l));
            assert_eq!(ours, theirs, "{n}");
        }
    }

    /// A Fixnum built from its bits is the one `long2fix` makes, and reads
    /// back as the number it was made from.
    #[test]
    fn a_fixnum_is_long2fix() {
        let api = get_default();
        let max = i64::from(rb_sys::special_consts::FIXNUM_MAX);
        let min = i64::from(rb_sys::special_consts::FIXNUM_MIN);
        for n in [0, 1, -1, 42, -42, max, min] {
            let l = fixable(n).unwrap();
            let v = fixnum(n).unwrap();
            assert_eq!(v, api.long2fix(l), "{n}");
            assert_eq!(i64::from(api.fix2long(v)), n, "{n}");
        }
        assert_eq!(posfixnum(max as u64), fixnum(max));
        assert_eq!(posfixnum(max as u64 + 1), None);
        assert_eq!(fixnum(max + 1), None);
        assert_eq!(fixnum(min - 1), None);
    }
}
