//! XPath 1.0 Number parsing (mkr_xpath_number.c).
//!
//! The production is `Digits ('.' Digits?)? | '.' Digits` - no sign, no
//! exponent, no hex, decimal point only. The C version had to scan the exact
//! grammar first and then fight `strtod`, which accepts a superset and honours
//! LC_NUMERIC; it carried a hand-rolled digit assembler as the fallback for a
//! comma-decimal locale. Rust's `f64` parser is locale-independent and
//! correctly rounded, so the scan below is still the grammar gate but the
//! conversion has no fallback to get wrong.

use core::ffi::c_char;

#[inline]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// Byte length of the longest prefix of `s` matching the Number production, or
/// 0 if it does not begin with one ("5." IS a Number, a bare "." is NOT).
pub fn extent(s: &[u8]) -> usize {
    let mut i = 0;
    if s.first().copied().is_some_and(is_digit) {
        /* Digits ('.' Digits?)?  -> "5", "5.", "5.5" */
        while s.get(i).copied().is_some_and(is_digit) {
            i += 1;
        }
        if s.get(i) == Some(&b'.') {
            i += 1;
            while s.get(i).copied().is_some_and(is_digit) {
                i += 1;
            }
        }
        return i;
    }
    if s.first() == Some(&b'.') {
        /* '.' Digits  -> ".5" */
        if !s.get(1).copied().is_some_and(is_digit) {
            return 0;
        }
        i = 1;
        while s.get(i).copied().is_some_and(is_digit) {
            i += 1;
        }
        return i;
    }
    0
}

/// Convert bytes the caller has already confirmed match the grammar. Those are
/// ASCII digits and at most one '.', which `f64::from_str` accepts in exactly
/// this shape, so the parse cannot fail; NaN is returned for an empty slice the
/// same way the C version did.
pub fn from_extent(s: &[u8]) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    match core::str::from_utf8(s) {
        Ok(t) => t.parse::<f64>().unwrap_or(f64::NAN),
        Err(_) => f64::NAN,
    }
}

/* ---- the C ABI (the string->number coercion in mkr_xpath_value_body.h calls
 * these; the lexer below is Rust now and calls the functions above) ---- */

#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_number_extent(p: *const c_char, len: usize) -> usize {
    if p.is_null() || len == 0 {
        return 0;
    }
    extent(core::slice::from_raw_parts(p as *const u8, len))
}

#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_number_from_extent(p: *const c_char, extent: usize) -> f64 {
    if p.is_null() || extent == 0 {
        return f64::NAN;
    }
    from_extent(core::slice::from_raw_parts(p as *const u8, extent))
}
