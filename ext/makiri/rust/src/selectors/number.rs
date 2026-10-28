//! The value of a CSS number: "convert a string to a number" of CSS Syntax,
//! the digits' exact value times ten to the exponent, as the nearest double.
//!
//! Lexbor's own conversion (`strtod.rs`, which only the differential
//! harnesses build) is not correctly rounded, so the two can differ in the
//! last bit for a digit run too long for an exact double; the Standard's
//! value is the one this gives (C29).

/// The longest text built: up to 128 digits (the tokenizer's buffer), `e`, a
/// sign and the ten digits of an `i32`.
const MAX_TEXT: usize = 128 + 1 + 1 + 10;

/// The value of the decimal digits `digits` times ten to `exp`: the nearest
/// double (C29), or - in the differential harnesses, inside
/// [`crate::compat::as_lexbor`] - Lexbor's own conversion.
pub fn value(digits: &[u8], exp: i32) -> f64 {
    #[cfg(any(test, feature = "lexbor-reference"))]
    if !crate::compat::fixed(crate::compat::Fix::C29) {
        return super::strtod::strtod_internal(digits, exp);
    }
    standard_value(digits, exp)
}

/// The nearest double to the decimal digits `digits` times ten to `exp`;
/// infinity past the double range, zero below it.
fn standard_value(digits: &[u8], exp: i32) -> f64 {
    if digits.is_empty() || digits.iter().all(|&d| d == b'0') {
        return 0.0;
    }
    let mut text = [0u8; MAX_TEXT];
    let mut n = 0;
    for &d in digits.iter().take(128) {
        text[n] = d;
        n += 1;
    }
    text[n] = b'e';
    n += 1;
    let mut magnitude = i64::from(exp).unsigned_abs();
    if exp < 0 {
        text[n] = b'-';
        n += 1;
    }
    let mut rev = [0u8; 10];
    let mut len = 0;
    loop {
        rev[len] = b'0' + (magnitude % 10) as u8;
        len += 1;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        text[n] = rev[len];
        n += 1;
    }
    /* Digits and an exponent always parse; were that not so, no value. */
    core::str::from_utf8(&text[..n])
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::standard_value;

    #[test]
    fn plain_values() {
        assert_eq!(standard_value(b"0", 0), 0.0);
        assert_eq!(standard_value(b"000", 5), 0.0);
        assert_eq!(standard_value(b"42", 0), 42.0);
        assert_eq!(standard_value(b"15", -1), 1.5);
        assert_eq!(standard_value(b"1", 400), f64::INFINITY);
        assert_eq!(standard_value(b"1", -400), 0.0);
    }

    #[test]
    fn it_is_correctly_rounded() {
        // 2^53 + 1 is halfway between two doubles; ties go to even.
        assert_eq!(standard_value(b"9007199254740993", 0), 9007199254740992.0);
        assert_eq!(standard_value(b"9007199254740995", 0), 9007199254740996.0);
    }

    #[test]
    fn exponents_at_the_int_range() {
        assert_eq!(standard_value(b"123456789", i32::MAX), f64::INFINITY);
        assert_eq!(standard_value(b"123456789", i32::MIN), 0.0);
    }

    /// The experiment behind C29: how often Lexbor's conversion and the
    /// standard one differ, for integers (what An+B reads) and for any
    /// decimal.
    #[test]
    #[ignore = "a measurement"]
    fn how_often_lexbors_value_differs() {
        use crate::selectors::strtod::strtod_internal;
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let (mut int_total, mut int_diff, mut dec_total, mut dec_diff) = (0, 0, 0, 0);
        let (mut small_long_diff, mut first_long) = (0, None);
        let mut first_int = None;
        for _ in 0..2_000_000 {
            let len = 1 + (next() % 24) as usize;
            let digits: Vec<u8> = (0..len).map(|_| b'0' + (next() % 10) as u8).collect();
            int_total += 1;
            let (a, b) = (strtod_internal(&digits, 0), standard_value(&digits, 0));
            if a != b {
                int_diff += 1;
                first_int.get_or_insert((String::from_utf8_lossy(&digits).into_owned(), a, b));
            }
            let exp = (next() % 40) as i32 - 20;
            dec_total += 1;
            let (da, db) = (strtod_internal(&digits, exp), standard_value(&digits, exp));
            if da != db {
                dec_diff += 1;
                if (da as i64) != (db as i64) && db.abs() < 9.0e15 {
                    small_long_diff += 1;
                    first_long.get_or_insert((
                        String::from_utf8_lossy(&digits).into_owned(),
                        exp,
                        da,
                        db,
                    ));
                }
            }
        }
        std::println!("integers: {int_diff}/{int_total} differ, first {first_int:?}");
        std::println!("decimals: {dec_diff}/{dec_total} differ; truncated values differ below 9e15: {small_long_diff}, first {first_long:?}");
    }
}
