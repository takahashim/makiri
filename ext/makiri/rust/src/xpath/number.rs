//! The XPath 1.0 Number production, read and written (mkr_xpath_number.c for
//! the read; the write is `string()`'s number rule, §4.2).
#![forbid(unsafe_code)]
//!
//! Both halves of one grammar, so they sit together: a change to what counts as
//! a Number is a change to both.
//!
//! The production is `Digits ('.' Digits?)? | '.' Digits` - no sign, no
//! exponent, no hex, decimal point only. The C version had to scan the exact
//! grammar first and then fight `strtod`, which accepts a superset and honours
//! LC_NUMERIC; it carried a hand-rolled digit assembler as the fallback for a
//! comma-decimal locale. Rust's `f64` parser is locale-independent and
//! correctly rounded, so the scan below is still the grammar gate but the
//! conversion has no fallback to get wrong.

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

/* ---- the write half: `string(number)` (§4.2) ---- */

/// A bounded `core::fmt::Write` sink. An overflow is an error rather than a
/// truncation: a cut-short number string is a wrong answer, and the C checked
/// its `snprintf` return for exactly that reason.
struct Fixed<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for Fixed<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if s.len() > self.buf.len() - self.len {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}

impl<'a> Fixed<'a> {
    fn new(buf: &'a mut [u8]) -> Fixed<'a> {
        Fixed { buf, len: 0 }
    }
    fn written(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Trailing zeros (and a bare trailing '.') dropped from a decimal run, which is
/// what `%g` does. A run with no '.' is returned unchanged.
fn strip_zeros(s: &[u8]) -> &[u8] {
    if !s.contains(&b'.') {
        return s;
    }
    let s = &s[..s.len() - s.iter().rev().take_while(|&&b| b == b'0').count()];
    s.strip_suffix(b".").unwrap_or(s)
}

/// Write `d` the way XPath's `string()` does (§4.2): an integral value in
/// range prints as an integer, everything else as C's `%.15g`.
///
/// Returns the byte length, or None if `out` was too small - which the caller
/// turns into an INTERNAL error rather than emitting a truncated number.
/// Allocation-free: this is on the value path of an engine that reports OOM as a
/// status, so it must not be able to abort on a failed allocation instead.
pub fn to_text(d: f64, out: &mut [u8]) -> Option<usize> {
    use core::fmt::Write;
    const P: i32 = 15;

    if d == d.trunc() && d.abs() < 1e15 {
        let mut w = Fixed::new(out);
        write!(w, "{}", d as i64).ok()?;
        return Some(w.len);
    }

    /* %.15g picks exponential when the decimal exponent is below -4 or at least
     * the precision, and strips trailing zeros either way. */
    let exp = if d == 0.0 {
        0
    } else {
        d.abs().log10().floor() as i32
    };
    let mut scratch = [0u8; 64];

    if (-4..P).contains(&exp) {
        let mut w = Fixed::new(&mut scratch);
        write!(w, "{:.*}", (P - 1 - exp).max(0) as usize, d).ok()?;
        let text = strip_zeros(w.written());
        if text.len() > out.len() {
            return None;
        }
        out[..text.len()].copy_from_slice(text);
        return Some(text.len());
    }

    let mut w = Fixed::new(&mut scratch);
    write!(w, "{:.*e}", (P - 1) as usize, d).ok()?;
    /* Rust writes "1.5e20"; C writes "1.5e+20". */
    let written = w.written();
    let at = written.iter().position(|&b| b == b'e')?;
    let mantissa = strip_zeros(&written[..at]);
    let ev: i32 = core::str::from_utf8(&written[at + 1..])
        .ok()?
        .parse()
        .ok()?;

    let mut o = Fixed::new(out);
    o.write_str(core::str::from_utf8(mantissa).ok()?).ok()?;
    write!(o, "e{}{:02}", if ev < 0 { '-' } else { '+' }, ev.abs()).ok()?;
    Some(o.len)
}
