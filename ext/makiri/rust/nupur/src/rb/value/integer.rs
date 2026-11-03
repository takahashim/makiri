//! An Integer read as a Rust number - checked, never cut - or written as
//! decimal digits: a Fixnum from its own bits, a Bignum by Ruby.

#![allow(unsafe_code)]

use nupur_sys::non_raising;

use super::{Integer, RString};
use crate::rb::error::{Builtin, Error};
use crate::rb::ruby::Ruby;

impl Integer {
    /// As a `u64`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_u64(self) -> Result<u64, Error> {
        if self.is_negative() {
            return Err(Error::new(
                Builtin::RangeError,
                "can't convert negative integer to unsigned",
            ));
        }
        if non_raising::fixnum_p(self.0 .0) {
            /* Not negative, checked above. */
            return u64::try_from(non_raising::fix2long(self.0 .0))
                .map_err(|_| crate::rb::error::internal_error("a negative Fixnum"));
        }
        crate::raw::big2ull(self)
    }

    /// As an `i64`; a value out of its range is Ruby's `RangeError`.
    #[inline]
    pub fn to_i64(self) -> Result<i64, Error> {
        if non_raising::fixnum_p(self.0 .0) {
            return Ok(non_raising::fix2long(self.0 .0));
        }
        crate::raw::big2ll(self)
    }

    /// As a `usize`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_usize(self) -> Result<usize, Error> {
        /* Through `to_u64`, so a value past a narrower `usize` is refused
         * rather than truncated. */
        usize::try_from(self.to_u64()?).map_err(|_| {
            Error::new(
                Builtin::RangeError,
                "integer too big to convert into a usize",
            )
        })
    }

    /// As a `usize`, or `None` when it is negative or past one - and only
    /// then: any other failure of the conversion is passed on, never read as
    /// "out of range" (for a limit, that would lift it). "Past one" is
    /// decided from the value's size, so no `RangeError` is made and dropped.
    pub fn to_usize_within(self) -> Result<Option<usize>, Error> {
        if self.is_negative() {
            return Ok(None);
        }
        if !non_raising::fixnum_p(self.0 .0) && !self.magnitude_fits_u64() {
            return Ok(None);
        }
        Ok(usize::try_from(self.to_u64()?).ok())
    }

    /// Whether a Bignum's magnitude fits a `u64`: `rb_absint_size`, which
    /// reads its digits and raises nothing - so "too large" is answered
    /// without making the `RangeError` `rb_big2ull` would raise.
    #[inline]
    fn magnitude_fits_u64(self) -> bool {
        // SAFETY: `self` is a live Integer, by type.
        let bytes = unsafe { non_raising::absint_size_of_integer(self.0 .0) };
        u64::try_from(bytes).is_ok_and(|b| b <= 8)
    }

    /// The decimal digits, with a `-` for a negative value, as a new
    /// String: a Fixnum written here, a Bignum by Ruby (`rb_big2str`). A
    /// failed allocation is `NoMemoryError`.
    pub fn to_decimal(self, ruby: &Ruby) -> Result<RString, Error> {
        if non_raising::fixnum_p(self.0 .0) {
            let mut buf = [0u8; 24];
            let n = write_i64(non_raising::fix2long(self.0 .0), &mut buf);
            /* Digits and a sign: ASCII, which `from_utf8` confirms in a step. */
            let digits = core::str::from_utf8(buf.get(..n).unwrap_or_default())
                .map_err(|_| crate::rb::error::internal_error("a Fixnum's digits are not ASCII"))?;
            return crate::string::utf8_str(ruby, digits);
        }
        crate::raw::big2str(self)
    }

    /// Below zero? Reads the sign, converts nothing.
    #[inline]
    pub fn is_negative(self) -> bool {
        if non_raising::fixnum_p(self.0 .0) {
            return non_raising::fix2long(self.0 .0) < 0;
        }
        // SAFETY: a non-Fixnum Integer is a live Bignum.
        unsafe { non_raising::bignum_negative_p(self.0 .0) }
    }
}

/// `n` in decimal into `buf`, which holds any `i64` (20 digits and a sign);
/// how many bytes.
fn write_i64(n: i64, buf: &mut [u8; 24]) -> usize {
    let mut digits = [0u8; 20];
    let mut m = n.unsigned_abs();
    let mut k = 0;
    loop {
        if let Some(d) = digits.get_mut(k) {
            *d = b'0' + u8::try_from(m % 10).unwrap_or(0);
        }
        k += 1;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    let mut at = 0;
    if n < 0 {
        if let Some(sign) = buf.first_mut() {
            *sign = b'-';
        }
        at = 1;
    }
    for d in digits.iter().take(k).rev() {
        if let Some(slot) = buf.get_mut(at) {
            *slot = *d;
        }
        at += 1;
    }
    at
}
