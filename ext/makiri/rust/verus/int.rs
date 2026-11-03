//! Verus proofs over Nupur's integer conversions (`nupur/sys/src/int.rs`), for
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
//! - `capacity(n)` is `min(n, LONG_MAX)`;
//! - `flonum_bits(t)`, where a `VALUE` is 64 bits, answers `Some` exactly
//!   where `rb_float_new_inline` makes a flonum (an exponent whose top three
//!   bits are 011 or 100, but the one colliding value, and +0.0); a `Some`
//!   carries the flonum tag (low bits `10`), so Ruby never reads it as a
//!   pointer, a Fixnum or a special constant; and Ruby's own decoding
//!   (`rb_float_flonum_value`, `flonum_value` below) gives `t` back.
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

/// Ruby's `rb_float_flonum_value` (its `ruby/internal/special_consts.h`),
/// over bits: the double the flonum `v` stands for. Ruby writes the bits it
/// restores as `2 - b63`, for `b63` the top bit, 0 or 1; that is written
/// here as the choice it makes.
pub open spec fn flonum_value(v: u64) -> u64 {
    if v != 0x8000_0000_0000_0002u64 {
        let x = (if v >> 63u64 == 0u64 { 2u64 } else { 1u64 }) | (v & !3u64);
        (x >> 3u64) | (x << 61u64)
    } else {
        0u64
    }
}

/// The flonum of a double, where a `VALUE` is 64 bits (Ruby makes flonums
/// only there): the copy of `flonum_bits`.
pub mod flonum64 {
    use vstd::prelude::*;
    use crate::rb_sys;

    pub fn flonum_bits(t: u64) -> (r: Option<rb_sys::VALUE>)
        ensures // [verus]
            r.is_some() <==> (t == 0 || (t != 0x3000_0000_0000_0000 // [verus]
                && ((t >> 60) & 0x7 == 3 || (t >> 60) & 0x7 == 4))), // [verus]
            r.is_some() ==> r.unwrap() & 3 == 2, // [verus]
            r.is_some() ==> crate::flonum_value(r.unwrap()) == t, // [verus]
    {
        let exp = (t >> 60) & 0x7;
        if t != 0x3000_0000_0000_0000 && (exp == 3 || exp == 4) {
            // [canary]
            #[allow(
                clippy::manual_rotate,
                reason = "Verus has no specification of rotate_left; the shifts are what verus/int.rs proves"
            )]
            let v = (((t << 3) | (t >> 61)) & !0x1) | 0x2;
            proof { crate::flonum_round_trip(t, v); } // [verus]
            return Some(v as rb_sys::VALUE);
        }
        if t == 0 {
            // [canary]
            proof { crate::flonum_zero(); } // [verus]
            return Some(0x8000_0000_0000_0002_u64 as rb_sys::VALUE);
        }
        // [canary]
        None
    }
}

/// An immediate-range double's flonum: the low two bits are the flonum
/// tag `10` - not a pointer (`00`), not a Fixnum (`x1`) - it is not +0.0's
/// own value, and Ruby's decoding turns it back into `t`: the two bits the
/// rotation dropped (62, 61) are 01 for exponent 011 and 10 for 100, which
/// bit 60 - now the top bit - tells apart.
proof fn flonum_round_trip(t: u64, v: u64)
    by (bit_vector)
    requires
        t != 0x3000_0000_0000_0000u64,
        (t >> 60u64) & 7u64 == 3u64 || (t >> 60u64) & 7u64 == 4u64,
        v == ((((t << 3u64) | (t >> 61u64)) & !1u64) | 2u64),
    ensures
        v & 3u64 == 2u64,
        v != 0x8000_0000_0000_0002u64,
        ((((if v >> 63u64 == 0u64 { 2u64 } else { 1u64 }) | (v & !3u64)) >> 3u64)
            | (((if v >> 63u64 == 0u64 { 2u64 } else { 1u64 }) | (v & !3u64)) << 61u64)) == t,
{
}

/// +0.0's flonum carries the tag too.
proof fn flonum_zero()
    by (bit_vector)
    ensures
        0x8000_0000_0000_0002u64 & 3u64 == 2u64,
{
}

} // verus!

fn main() {}
