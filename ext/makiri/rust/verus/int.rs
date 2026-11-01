//! Verus proofs over Nupur's integer conversions (`nupur/src/int.rs`), for
//! every input and at both widths of a C `long`: 32 bits (Windows, LLP64)
//! and 64 (everywhere else). `c_long` is `i64` where the code is built and
//! linted, so a truncation that happens only on Windows is invisible to
//! rustc and clippy; the tests check a dozen points near each bound, and
//! these proofs every one.
//!
//! Write `FIXMIN = MIN / 2` and `FIXMAX = MAX / 2` for a `long` (Rust's `/`
//! rounds toward zero). Proved over the shipped code, at each width:
//!
//! - `fixable_in` answers `Some(l)` exactly when `FIXMIN <= n <= FIXMAX`, and
//!   then `l == n`;
//! - `posfixable_in` the same for a `u64` (`None` past `i64::MAX`);
//! - `fixnum_bits(l)` sets the Fixnum flag (its low bit is 1), and for a
//!   Fixnum's `l` an arithmetic shift right by one gives `l` back: every
//!   32-bit `long`, and a 64-bit one in `FIXMIN..=FIXMAX` - past it `2 l + 1`
//!   loses its top bit, which is why `fixnum` asks `fixable` first;
//! - `len(n)` is `n`, or 0 for a negative `n`;
//! - `capacity(n)` is `min(n, LONG_MAX)`.
//!
//! `fixable`, `posfixable`, `fixnum` and `posfixnum` are these at this
//! target's `c_long`. `usize` is 64 bits here, as on every target Makiri
//! builds for.
//!
//! How the copies are made: each width is a module that names the width
//! `type L` and `type c_long`, and holds the copies, whose bodies are the
//! source's token for token (`rake verus:sync` checks every copy, in both
//! modules). The source's `fixable_in<L: Long>` is copied as
//! `fixable_in<M>`, whose body's `L` is the module's: the parameter only
//! lets `posfixable_in`'s `fixable_in::<L>` name it.
//!
//! Not proved here: that `fixnum_bits` is Ruby's `LONG2FIX`, or this
//! target's bounds rb-sys's `FIXNUM_MIN` / `FIXNUM_MAX` - Ruby's C has no
//! contract. The tests `a_fixnum_is_long2fix` and
//! `this_targets_long_agrees_with_rb_sys` keep checking those, and
//! `FIXNUM_FLAG` is taken as 1 (`rb_sys`, modelled below).
//!
//! `fixnum_bits`' cast is written `#[verifier::truncate] (l as VALUE)` in
//! the copy: the attribute tells Verus the cast wraps, as `as` does, and
//! `rake verus:sync` drops it with its parentheses.
//!
//! The axiom: std's `Result::unwrap_or` (the value, or the default), which
//! vstd does not state.

use vstd::prelude::*;

verus! {

global size_of usize == 8;

/// `Result::unwrap_or`: the `Ok` value, else `default` - as std documents
/// it.
pub assume_specification<T, E>[ Result::<T, E>::unwrap_or ](result: Result<T, E>, default: T) -> (t: T)
    ensures
        t == match result {
            Ok(v) => v,
            Err(_) => default,
        },
;

/// The two items of rb-sys `int.rs` reads, as rb-sys defines them: a
/// `VALUE` is a 64-bit word, and the Fixnum flag is bit 0.
pub mod rb_sys {
    pub type VALUE = u64;

    pub const FIXNUM_FLAG: u64 = 1;
}

/// A `long` of 32 bits: Windows.
pub mod long32 {
    use vstd::prelude::*;
    use crate::rb_sys;

    #[allow(non_camel_case_types)]
    pub type c_long = i32;

    pub type L = i32;

    pub fn fixable_in<M>(n: i64) -> (r: Option<L>)
        ensures // [verus]
            r.is_some() <==> i32::MIN / 2 <= n <= i32::MAX / 2, // [verus]
            r.is_some() ==> r.unwrap() as int == n, // [verus]
    {
        let (min, max) = (Into::<i64>::into(L::MIN) / 2, Into::<i64>::into(L::MAX) / 2);
        if n < min || n > max {
            // [canary]
            return None;
        }
        // [canary]
        L::try_from(n).ok()
    }

    pub fn posfixable_in<M>(n: u64) -> (r: Option<L>)
        ensures // [verus]
            r.is_some() <==> n <= i32::MAX / 2, // [verus]
            r.is_some() ==> r.unwrap() as int == n, // [verus]
    {
        // [canary]
        i64::try_from(n).ok().and_then(fixable_in::<L>)
    }

    pub fn fixnum_bits(l: c_long) -> (v: rb_sys::VALUE)
        ensures // [verus]
            v & 1 == 1, // [verus]
            (#[verifier::truncate] (v as i64)) >> 1u64 == l as i64, // [verus]
    {
        // [canary]
        let r = // [verus]
        ((#[verifier::truncate] (l as rb_sys::VALUE)) << 1) | rb_sys::FIXNUM_FLAG as rb_sys::VALUE
        ; // [verus]
        proof { crate::bits32(l, r); } // [verus]
        r // [verus]
    }

    pub fn len(n: c_long) -> (r: usize)
        ensures // [verus]
            r == (if n >= 0 { n as int } else { 0 }), // [verus]
    {
        // [canary]
        usize::try_from(n).unwrap_or(0)
    }

    pub fn capacity(n: usize) -> (r: c_long)
        ensures // [verus]
            r == (if n <= c_long::MAX { n as int } else { c_long::MAX as int }), // [verus]
    {
        // [canary]
        c_long::try_from(n).unwrap_or(c_long::MAX)
    }
}

/// A `long` of 64 bits: Linux and macOS.
pub mod long64 {
    use vstd::prelude::*;
    use crate::rb_sys;

    #[allow(non_camel_case_types)]
    pub type c_long = i64;

    pub type L = i64;

    pub fn fixable_in<M>(n: i64) -> (r: Option<L>)
        ensures // [verus]
            r.is_some() <==> i64::MIN / 2 <= n <= i64::MAX / 2, // [verus]
            r.is_some() ==> r.unwrap() as int == n, // [verus]
    {
        let (min, max) = (Into::<i64>::into(L::MIN) / 2, Into::<i64>::into(L::MAX) / 2);
        if n < min || n > max {
            // [canary]
            return None;
        }
        // [canary]
        L::try_from(n).ok()
    }

    pub fn posfixable_in<M>(n: u64) -> (r: Option<L>)
        ensures // [verus]
            r.is_some() <==> n <= i64::MAX / 2, // [verus]
            r.is_some() ==> r.unwrap() as int == n, // [verus]
    {
        // [canary]
        i64::try_from(n).ok().and_then(fixable_in::<L>)
    }

    pub fn fixnum_bits(l: c_long) -> (v: rb_sys::VALUE)
        ensures // [verus]
            v & 1 == 1, // [verus]
            i64::MIN / 2 <= l <= i64::MAX / 2 // [verus]
                ==> (#[verifier::truncate] (v as i64)) >> 1u64 == l, // [verus]
    {
        // [canary]
        let r = // [verus]
        ((#[verifier::truncate] (l as rb_sys::VALUE)) << 1) | rb_sys::FIXNUM_FLAG as rb_sys::VALUE
        ; // [verus]
        proof { crate::bits64(l, r); } // [verus]
        r // [verus]
    }

    pub fn len(n: c_long) -> (r: usize)
        ensures // [verus]
            r == (if n >= 0 { n as int } else { 0 }), // [verus]
    {
        // [canary]
        usize::try_from(n).unwrap_or(0)
    }

    pub fn capacity(n: usize) -> (r: c_long)
        ensures // [verus]
            r == (if n <= c_long::MAX { n as int } else { c_long::MAX as int }), // [verus]
    {
        // [canary]
        c_long::try_from(n).unwrap_or(c_long::MAX)
    }
}

/// A 32-bit `long`'s Fixnum: `as` sign-extends it to 64 bits, where every
/// `i32` is in a Fixnum's range, so the shift back always gives it.
proof fn bits32(l: i32, r: u64)
    by (bit_vector)
    requires
        r == ((#[verifier::truncate] (l as u64)) << 1u64) | 1u64,
    ensures
        r & 1 == 1,
        (#[verifier::truncate] (r as i64)) >> 1u64 == l as i64,
{
}

/// A 64-bit `long`'s Fixnum: the shift back gives `l` exactly when `2 l + 1`
/// fits, which is a Fixnum's range - past it the top bit is lost, which is
/// why `fixnum` asks `fixable` first.
proof fn bits64(l: i64, r: u64)
    by (bit_vector)
    requires
        r == ((#[verifier::truncate] (l as u64)) << 1u64) | 1u64,
    ensures
        r & 1 == 1,
        -0x4000_0000_0000_0000i64 <= l <= 0x3fff_ffff_ffff_ffffi64
            ==> (#[verifier::truncate] (r as i64)) >> 1u64 == l,
{
}

} // verus!

fn main() {}
