//! Verus proofs over the XML reference expander
//! (`engine/src/xml/chars/expand.rs`), for inputs of every length.
//!
//! - `scan_char_ref` reads a numeric character reference (`&#...;`, after the
//!   `&#`) exactly: it answers `Ok` exactly when the digits run to a ';', there
//!   is at least one, and their value - an unbounded integer here - is at most
//!   U+10FFFF and an XML `Char`; and then with that value. The overflow guard
//!   is what keeps a giant reference from wrapping into the valid range, and
//!   this is the proof it does.
//! - `expand_into` never answers `Overflow` when `out` is as long as `src`,
//!   and writes at most `src.len()` bytes: what it writes for each piece of
//!   input is never longer than the piece (a character as itself or one
//!   space; `&#...;`, four bytes at least, as at most four; `&name;` as one
//!   byte). The source documents this "output <= input" as an invariant whose
//!   breaking would be an internal error; here it is a theorem.
//!
//! Proved over the shipped code (each function is copied, and
//! `rake verus:sync` checks it against the source). `walk` is proved at the
//! instance `expand_into` calls - writing through a `Writer`, with an `Fn`
//! for the names - by giving the copy those parameter types; its body is the
//! source's, token for token. (`only_unexpanded`'s instance, writing
//! nowhere, has no length to bound.)
//!
//! The axioms: `decode1`'s answer is at least one byte and inside its input
//! (`verus/utf8.rs` proves that and more of the same code); `validate_name`
//! and `predefined_entity` return - nothing is assumed of what.

use vstd::prelude::*;

verus! {

pub open spec fn in_range(c: u32, lo: u32, hi: u32) -> bool {
    lo <= c && c <= hi
}

/// XML 1.0 §2.2 [2] Char.
pub open spec fn char_spec(c: u32) -> bool {
    c == 0x9 || c == 0xA || c == 0xD || in_range(c, 0x20, 0xD7FF) || in_range(c, 0xE000, 0xFFFD)
        || in_range(c, 0x10000, 0x10FFFF)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExpandErr {
    Syntax,
    Overflow,
}

pub fn is_char(c: u32) -> (r: bool)
    ensures // [verus]
        r == char_spec(c), // [verus]
{
    c == 0x9
        || c == 0xA
        || c == 0xD
        || (0x20..=0xD7FF).contains(&c)
        || (0xE000..=0xFFFD).contains(&c)
        || (0x10000..=0x10FFFF).contains(&c)
}

/// The reference is hexadecimal: it begins with a lowercase 'x' (§4.1).
pub open spec fn hexp(s: Seq<u8>) -> bool {
    s.len() > 0 && s[0] == 0x78
}

/// Where its digits begin.
pub open spec fn start(s: Seq<u8>) -> int {
    if hexp(s) { 1 } else { 0 }
}

pub open spec fn radix(s: Seq<u8>) -> int {
    if hexp(s) { 16 } else { 10 }
}

/// A digit's value in the reference's base, -1 for a byte that is none.
pub open spec fn dval(b: u8, hex: bool) -> int {
    if 0x30 <= b <= 0x39 {
        b - 0x30
    } else if hex && 0x61 <= b <= 0x66 {
        b - 0x61 + 10
    } else if hex && 0x41 <= b <= 0x46 {
        b - 0x41 + 10
    } else {
        -1
    }
}

pub open spec fn is_digit(s: Seq<u8>, i: int) -> bool {
    dval(s[i], hexp(s)) >= 0
}

/// Where the run of digits from `k` ends: the first non-digit, or the end.
pub open spec fn run_end(s: Seq<u8>, k: int) -> int
    decreases s.len() - k,
{
    if k >= s.len() || k < 0 {
        k
    } else if !is_digit(s, k) {
        k
    } else {
        run_end(s, k + 1)
    }
}

/// The value of the digits `start(s)..k`, as an integer: no width, no wrap.
pub open spec fn value(s: Seq<u8>, k: int) -> int
    decreases k,
{
    if k <= start(s) {
        0
    } else {
        value(s, k - 1) * radix(s) + dval(s[k - 1], hexp(s))
    }
}

/// The reference is well-formed and legal: its digits, at least one, run to a
/// ';', and name a `Char`.
pub open spec fn char_ref(s: Seq<u8>) -> bool {
    let e = run_end(s, start(s));
    e < s.len() && s[e] == 0x3b && e > start(s) && value(s, e) <= 0x10FFFF && char_spec(
        value(s, e) as u32,
    )
}

fn scan_char_ref(src: &[u8]) -> (r: Result<(u32, usize), ExpandErr>)
    ensures // [verus]
        r.is_err() ==> r == Err::<(u32, usize), ExpandErr>(ExpandErr::Syntax), // [verus]
        r.is_ok() <==> char_ref(src@), // [verus]
        r.is_ok() ==> r == Ok::<(u32, usize), ExpandErr>( // [verus]
            (value(src@, run_end(src@, start(src@))) as u32, // [verus]
            (run_end(src@, start(src@)) + 1) as usize)), // [verus]
{
    let ghost s = src@; // [verus]
    let mut i = 0usize;
    /* §4.1: the hex marker is a lowercase 'x' only. */
    let hex = src.first() == Some(&b'x');
    if hex {
        i += 1;
    }
    let base: u32 = if hex { 16 } else { 10 };
    let mut cp: u32 = 0;
    let mut ndigits = 0usize;
    proof { run_skip(s, start(s), start(s)); } // [verus]
    loop
        invariant // [verus]
            s == src@, // [verus]
            s.len() == src.len(), // [verus]
            hex == hexp(s), // [verus]
            base == radix(s), // [verus]
            start(s) <= i <= s.len(), // [verus]
            ndigits == i - start(s), // [verus]
            forall|k: int| start(s) <= k < i ==> is_digit(s, k), // [verus]
            run_end(s, start(s)) == run_end(s, i as int), // [verus]
            cp == value(s, i as int), // [verus]
            value(s, i as int) <= 0x10FFFF, // [verus]
        ensures // [verus]
            start(s) <= i <= s.len(), // [verus]
            ndigits == i - start(s), // [verus]
            run_end(s, start(s)) == i, // [verus]
            cp == value(s, i as int), // [verus]
        decreases s.len() - i, // [verus]
    {
        let d = match src.get(i) {
            None | Some(&b';') => break,
            Some(&d) => d,
        };
        let dig: u32 = match d {
            b'0'..=b'9' => (d - b'0') as u32,
            b'a'..=b'f' if hex => (d - b'a' + 10) as u32,
            b'A'..=b'F' if hex => (d - b'A' + 10) as u32,
            _ => return Err(ExpandErr::Syntax),
        };
        proof { // [verus]
            assert(i < s.len()); // [verus]
            assert(is_digit(s, i as int) && dval(s[i as int], hex) == dig); // [verus]
            run_skip(s, i as int, i + 1); // [verus]
        } // [verus]
        /* check BEFORE the multiply-add so a giant reference can never
         * wrap into the valid range */
        if cp > (0x10FFFF - dig) / base {
            proof { // [verus]
                guard(cp as int, dig as int, base as int); // [verus]
                run_facts(s, i + 1); // [verus]
                grows(s, i + 1, run_end(s, i + 1)); // [verus]
            } // [verus]
            // [canary]
            return Err(ExpandErr::Syntax);
        }
        proof { guard(cp as int, dig as int, base as int); } // [verus]
        cp = cp * base + dig;
        ndigits += 1;
        i += 1;
    }
    if src.get(i) != Some(&b';') || ndigits == 0 {
        // [canary]
        return Err(ExpandErr::Syntax);
    }
    if !is_char(cp) {
        // [canary]
        return Err(ExpandErr::Syntax);
    }
    // [canary]
    Ok((cp, i + 1))
}

/// The overflow guard is exact: `cp` past `(M - d) / b` is exactly
/// `cp * b + d` past `M`.
proof fn guard(cp: int, d: int, b: int)
    requires
        0 <= cp,
        0 <= d < b,
        b == 10 || b == 16,
    ensures
        cp > (0x10FFFF - d) / b <==> cp * b + d > 0x10FFFF,
{
    let q = (0x10FFFF - d) / b;
    let r = (0x10FFFF - d) % b;
    assert(0x10FFFF - d == q * b + r && 0 <= r < b) by (nonlinear_arith)
        requires
            0 <= d < b,
            b > 0,
            q == (0x10FFFF - d) / b,
            r == (0x10FFFF - d) % b;
    if cp > q {
        assert(cp * b >= (q + 1) * b) by (nonlinear_arith)
            requires
                cp >= q + 1,
                b > 0;
    } else {
        assert(cp * b <= q * b) by (nonlinear_arith)
            requires
                cp <= q,
                b > 0;
    }
}

/// Digits `i..k` do not move where the run ends.
proof fn run_skip(s: Seq<u8>, i: int, k: int)
    requires
        0 <= i <= k <= s.len(),
        forall|j: int| i <= j < k ==> is_digit(s, j),
    ensures
        run_end(s, i) == run_end(s, k),
    decreases k - i,
{
    if i < k {
        run_skip(s, i + 1, k);
    }
}

/// More digits never make a smaller value.
proof fn grows(s: Seq<u8>, a: int, b: int)
    requires
        start(s) <= a <= b <= s.len(),
        forall|j: int| start(s) <= j < b ==> is_digit(s, j),
    ensures
        value(s, a) <= value(s, b),
    decreases b - a,
{
    if a < b {
        grows(s, a, b - 1);
        nonneg(s, b - 1);
        let v = value(s, b - 1);
        let r = radix(s);
        assert(is_digit(s, b - 1));
        assert(v * r >= v) by (nonlinear_arith)
            requires
                v >= 0,
                r >= 10;
    }
}

proof fn nonneg(s: Seq<u8>, k: int)
    requires
        start(s) <= k <= s.len(),
        forall|j: int| start(s) <= j < k ==> is_digit(s, j),
    ensures
        value(s, k) >= 0,
    decreases k,
{
    if k > start(s) {
        nonneg(s, k - 1);
        let v = value(s, k - 1);
        let r = radix(s);
        assert(is_digit(s, k - 1));
        assert(v * r >= 0) by (nonlinear_arith)
            requires
                v >= 0,
                r >= 10;
    }
}

/// The run from `k` ends inside `s`, past `k`, and is digits up to there.
proof fn run_facts(s: Seq<u8>, k: int)
    requires
        0 <= k <= s.len(),
    ensures
        k <= run_end(s, k) <= s.len(),
        forall|j: int| k <= j < run_end(s, k) ==> is_digit(s, j),
        run_end(s, k) == s.len() || !is_digit(s, run_end(s, k)),
    decreases s.len() - k,
{
    if k < s.len() && is_digit(s, k) {
        run_facts(s, k + 1);
    }
}


// ---- the walk: what `expand_into` writes is never longer than it reads ----

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExpandMode {
    Text,
    Attr,
}

/// `decode1` (`engine/src/utf8_decode.rs`): a decoded scalar is at least one
/// byte and inside the input. `verus/utf8.rs` proves more of the same code
/// (exactly vstd's first scalar, whose length is 1 to 4 and within `p`); this
/// file states only the part the walk needs.
#[verifier::external_body]
pub fn decode1(p: &[u8]) -> (r: Option<(u32, usize)>)
    ensures
        r.is_some() ==> 1 <= r.unwrap().1 <= p@.len(),
{
    unimplemented!()
}

/// `validate_name` (`engine/src/xml/chars/mod.rs`): any answer will do here.
#[verifier::external_body]
pub fn validate_name(s: &[u8]) -> bool {
    unimplemented!()
}

/// `predefined_entity`: any answer will do here.
#[verifier::external_body]
fn predefined_entity(name: &[u8]) -> Option<u8> {
    unimplemented!()
}

pub struct Writer<'a> {
    pub out: &'a mut [u8],
    pub pos: usize,
}

impl<'a> Writer<'a> {
    fn put(&mut self, b: u8) -> (r: Result<(), ExpandErr>)
        ensures // [verus]
            final(self).out.len() == old(self).out.len(), // [verus]
            old(self).pos < old(self).out.len() ==> r.is_ok() && final(self).pos == old(self).pos + 1, // [verus]
    {
        match self.out.get_mut(self.pos) {
            Some(slot) => {
                *slot = b;
                assert(self.pos < self.out.len()); // [verus]
                self.pos += 1;
                Ok(())
            }
            None => Err(ExpandErr::Overflow),
        }
    }

    fn put_slice(&mut self, s: &[u8]) -> (r: Result<(), ExpandErr>)
        ensures // [verus]
            final(self).out.len() == old(self).out.len(), // [verus]
            old(self).pos + s.len() <= old(self).out.len() ==> r.is_ok() // [verus]
                && final(self).pos == old(self).pos + s.len(), // [verus]
    {
        let end = self.pos.checked_add(s.len()).ok_or(ExpandErr::Overflow)?;
        match self.out.get_mut(self.pos..end) {
            Some(dst) => {
                dst.copy_from_slice(s);
                self.pos = end;
                Ok(())
            }
            None => Err(ExpandErr::Overflow),
        }
    }
}

pub struct Utf8Char {
    pub buf: [u8; 4],
    pub len: usize,
}

impl Utf8Char {
    pub fn encode(cp: u32) -> (r: Utf8Char)
        ensures // [verus]
            1 <= r.len <= 4, // [verus]
    {
        let mut buf = [0u8; 4];
        let len = if cp < 0x80 {
            buf[0] = cp as u8;
            1
        } else if cp < 0x800 {
            buf[0] = 0xC0 | (cp >> 6) as u8;
            buf[1] = 0x80 | (cp & 0x3F) as u8;
            2
        } else if cp < 0x10000 {
            buf[0] = 0xE0 | (cp >> 12) as u8;
            buf[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            buf[2] = 0x80 | (cp & 0x3F) as u8;
            3
        } else {
            buf[0] = 0xF0 | (cp >> 18) as u8;
            buf[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
            buf[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            buf[3] = 0x80 | (cp & 0x3F) as u8;
            4
        };
        Utf8Char { buf, len }
    }

    pub fn as_bytes(&self) -> (r: &[u8])
        requires // [verus]
            self.len <= 4, // [verus]
        ensures // [verus]
            r@.len() == self.len, // [verus]
    {
        &self.buf[..self.len]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reference<'a> {
    Char(u32),
    Named(&'a [u8]),
}

pub fn scan_reference(src: &[u8]) -> (r: Result<(Reference<'_>, usize), ExpandErr>)
    ensures // [verus]
        r.is_err() ==> r == Err::<(Reference<'_>, usize), ExpandErr>(ExpandErr::Syntax), // [verus]
        r.is_ok() ==> r.unwrap().1 <= src.len(), // [verus]
        r.is_ok() ==> match r.unwrap().0 { // [verus]
            Reference::Char(_) => r.unwrap().1 >= 3, // [verus]
            Reference::Named(_) => r.unwrap().1 >= 1, // [verus]
        }, // [verus]
{
    if src.first() == Some(&b'#') {
        let (cp, used) = scan_char_ref(&src[1..])?;
        proof { char_ref_len(src@.subrange(1, src.len() as int)); } // [verus]
        // [canary]
        return Ok((Reference::Char(cp), used + 1));
    }
    /* The name runs to the first ';'. (A loop rather than `position`, which
     * Verus has no contract for: `verus/xml_expand.rs`.) */
    let mut nlen = 0;
    while nlen < src.len() && src[nlen] != b';'
        invariant // [verus]
            nlen <= src.len(), // [verus]
        decreases src.len() - nlen, // [verus]
    {
        nlen += 1;
    }
    if nlen == src.len() {
        return Err(ExpandErr::Syntax);
    }
    let name = &src[..nlen];
    /* The five predefined names are Names already, and they are nearly every
     * reference a document holds: the full Name check is for the rest. */
    if predefined_entity(name).is_none() && !validate_name(name) {
        return Err(ExpandErr::Syntax);
    }
    // [canary]
    Ok((Reference::Named(name), nlen + 1))
}

/// A well-formed character reference spans at least two bytes after its
/// `&#` - a digit and the ';'.
proof fn char_ref_len(s: Seq<u8>)
    requires
        char_ref(s),
    ensures
        run_end(s, start(s)) + 1 >= 2,
        run_end(s, start(s)) + 1 <= s.len(),
{
}

pub enum Named {
    Byte(u8),
    Unexpanded,
}

fn walk(
    src: &[u8],
    mode: ExpandMode,
    out: &mut Writer,
    named: impl Fn(&[u8]) -> Option<Named>,
) -> (r: Result<bool, ExpandErr>)
    requires // [verus]
        forall|n: &[u8]| named.requires((n,)), // [verus]
        old(out).pos == 0, // [verus]
        old(out).out.len() >= src.len(), // [verus]
    ensures // [verus]
        r != Err::<bool, ExpandErr>(ExpandErr::Overflow), // [verus]
        r.is_ok() ==> final(out).pos <= src.len(), // [verus]
{
    let mut unexpanded = false;
    let mut i = 0usize;
    while i < src.len()
        invariant // [verus]
            forall|n: &[u8]| named.requires((n,)), // [verus]
            i <= src.len(), // [verus]
            out.pos <= i, // [verus]
            out.out.len() >= src.len(), // [verus]
        decreases src.len() - i, // [verus]
    {
        if src[i] != b'&' {
            let (cp, bl) = decode1(&src[i..]).ok_or(ExpandErr::Syntax)?;
            if !is_char(cp) {
                return Err(ExpandErr::Syntax);
            }
            if mode == ExpandMode::Attr && (cp == 0x9 || cp == 0xA || cp == 0xD) {
                // [canary]
                out.put(b' ')?;
            } else {
                // [canary]
                out.put_slice(&src[i..i + bl])?;
            }
            i += bl;
            continue;
        }
        let (reference, used) = scan_reference(&src[i + 1..])?;
        i += 1 + used;
        match reference {
            Reference::Char(cp) => out.put_slice(Utf8Char::encode(cp).as_bytes())?,
            Reference::Named(name) => match named(name).ok_or(ExpandErr::Syntax)? {
                Named::Byte(b) => out.put(b)?,
                Named::Unexpanded => unexpanded = true,
            },
        }
    }
    // [canary]
    Ok(unexpanded)
}

pub fn expand_into(src: &[u8], mode: ExpandMode, out: &mut [u8]) -> (r: Result<usize, ExpandErr>)
    requires // [verus]
        old(out).len() >= src.len(), // [verus]
    ensures // [verus]
        r != Err::<usize, ExpandErr>(ExpandErr::Overflow), // [verus]
        r.is_ok() ==> r.unwrap() <= src.len(), // [verus]
{
    let mut w = Writer { out, pos: 0 };
    walk(src, mode, &mut w, |name| {
        let b = predefined_entity(name)?;
        Some(Named::Byte(b))
    })?;
    // [canary]
    Ok(w.pos)
}

} // verus!

fn main() {}
