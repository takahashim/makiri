//! Verus proofs over the line table `Node#line` reads
//! (`engine/src/source_lines.rs`'s `lines_build`), for inputs of every
//! length: the table is exactly 0 and the offset past each newline, in
//! order. A line number is nil rather than wrong; a table that missed a
//! newline, or saw one that is not there, would make every line after it
//! wrong.
//!
//! The scan reads sixteen bytes a step, tests each word with a SWAR mask, and
//! walks only the words holding a newline, bit by bit. Proved over the shipped
//! code (each function is copied, and `rake verus:sync` checks it against the
//! source):
//!
//! - `newline_mask` marks the high bit of exactly the bytes that are `\n`
//!   (it is the exact test, with no borrow between bytes);
//! - `push_starts` pushes the offset past each marked byte, lowest first;
//! - `lines_build` returns `starts_of(bytes)`, the table written from its
//!   definition one byte at a time.
//!
//! The axiom: falloc's `falloc_push` (the element appended, or `Err` with
//! nothing changed). `trailing_zeros` is vstd's, which vstd proves from its
//! definition.

use vstd::prelude::*;
use vstd::std_specs::bits::*;

verus! {

broadcast use axiom_u64_trailing_zeros;

pub mod falloc {
    use vstd::prelude::*;

    pub trait VecGrow<T> {
        fn falloc_push(&mut self, item: T) -> Result<(), ()>;
    }

    impl<T> VecGrow<T> for Vec<T> {
        /// `item` appended - or `Err` with nothing changed.
        #[verifier::external_body]
        fn falloc_push(&mut self, item: T) -> (r: Result<(), ()>)
            ensures
                r.is_ok() ==> final(self)@ == old(self)@.push(item),
                r.is_err() ==> final(self)@ == old(self)@,
        {
            unimplemented!()
        }
    }
}

use falloc::VecGrow;

pub struct Lines {
    pub starts: Vec<usize>,
}

/// The table, by definition: 0, then the offset past each `\n` of `s[..n]`.
pub open spec fn starts_of(s: Seq<u8>, n: int) -> Seq<usize>
    decreases n,
{
    if n <= 0 {
        seq![0usize]
    } else if s[n - 1] == 0x0a {
        starts_of(s, n - 1).push(n as usize)
    } else {
        starts_of(s, n - 1)
    }
}

/// Byte `j` of `w`, first byte lowest.
pub open spec fn byte(w: u64, j: u64) -> u64 {
    (w >> (8 * j) as u64) & 0xff
}

/// The high bit of byte `j` of `m` is set.
pub open spec fn marked(m: u64, j: u64) -> bool {
    (m >> (8 * j + 7) as u64) & 1 == 1
}

/// The offsets `push_starts` pushes for the marked bytes `lo..hi` of `m`, the
/// word at `at`.
pub open spec fn marks(m: u64, at: int, lo: int, hi: int) -> Seq<usize>
    decreases hi - lo,
{
    if hi <= lo {
        Seq::empty()
    } else if marked(m, (hi - 1) as u64) {
        marks(m, at, lo, hi - 1).push((at + hi) as usize)
    } else {
        marks(m, at, lo, hi - 1)
    }
}

pub fn word(b: &[u8]) -> (w: u64)
    requires // [verus]
        b.len() >= 8, // [verus]
    ensures // [verus]
        forall|j: u64| j < 8 ==> #[trigger] byte(w, j) == b@[j as int] as u64, // [verus]
{
    // [canary]
    let ghost g = b@; // [verus]
    let r = // [verus]
    u64::from(b[0])
        | u64::from(b[1]) << 8
        | u64::from(b[2]) << 16
        | u64::from(b[3]) << 24
        | u64::from(b[4]) << 32
        | u64::from(b[5]) << 40
        | u64::from(b[6]) << 48
        | u64::from(b[7]) << 56
    ; // [verus]
    proof { word_bytes(g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7], r); } // [verus]
    r // [verus]
}

fn newline_mask(w: u64) -> (m: u64)
    ensures // [verus]
        forall|j: u64| j < 8 ==> (#[trigger] marked(m, j) <==> byte(w, j) == 0x0a), // [verus]
        m & 0x7f7f_7f7f_7f7f_7f7f == 0, // [verus]
{
    const LOW7: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const NEWLINES: u64 = 0x0a0a_0a0a_0a0a_0a0a;
    let x = w ^ NEWLINES;
    // [canary]
    let r = // [verus]
    !((x & LOW7).wrapping_add(LOW7) | x | LOW7)
    ; // [verus]
    proof { // [verus]
        assert forall|j: u64| j < 8 implies (#[trigger] marked(r, j) <==> byte(w, j) == 0x0a) by { // [verus]
            mask_exact(w, j); // [verus]
        } // [verus]
        mask_high_only(w); // [verus]
    } // [verus]
    r // [verus]
}

pub fn lines_build(bytes: &[u8]) -> (r: Option<Lines>)
    ensures // [verus]
        r.is_some() ==> r.unwrap().starts@ == starts_of(bytes@, bytes@.len() as int), // [verus]
{
    let mut starts: Vec<usize> = Vec::new();
    starts.falloc_push(0).ok()?;
    let mut base = 0usize;
    let mut rest = bytes;
    while rest.len() >= 16
        invariant // [verus]
            base <= bytes.len(), // [verus]
            rest@ == bytes@.subrange(base as int, bytes@.len() as int), // [verus]
            starts@ == starts_of(bytes@, base as int), // [verus]
        decreases rest.len(), // [verus]
    {
        let lo = newline_mask(word(rest));
        let hi = newline_mask(word(&rest[8..]));
        proof { // [verus]
            word_marks(bytes@, base as int, lo); // [verus]
            word_marks(bytes@, base + 8, hi); // [verus]
        } // [verus]
        if lo | hi != 0 {
            // [canary]
            push_starts(&mut starts, base, lo)?;
            push_starts(&mut starts, base + 8, hi)?;
        } else { // [verus]
            proof { // [verus]
                both_clear(lo, hi); // [verus]
                no_marks(0, base as int, 0, 8); // [verus]
                no_marks(0, base + 8, 0, 8); // [verus]
            } // [verus]
        }
        rest = &rest[16..];
        base += 16;
    }
    let mut i = base;
    while i < bytes.len()
        invariant // [verus]
            base <= i <= bytes.len(), // [verus]
            starts@ == starts_of(bytes@, i as int), // [verus]
        decreases bytes.len() - i, // [verus]
    {
        if bytes[i] == b'\n' {
            // [canary]
            starts.falloc_push(i + 1).ok()?;
        }
        i += 1;
    }
    Some(Lines { starts })
}

fn push_starts(starts: &mut Vec<usize>, at: usize, mut m: u64) -> (r: Option<()>)
    requires // [verus]
        at + 8 <= usize::MAX, // [verus]
        m & 0x7f7f_7f7f_7f7f_7f7f == 0, // [verus]
    ensures // [verus]
        r.is_some() ==> final(starts)@ == old(starts)@ + marks(m, at as int, 0, 8), // [verus]
{
    let ghost m0 = m; // [verus]
    let ghost mut p: int = 0; // [verus]
    while m != 0
        invariant // [verus]
            0 <= p <= 8, // [verus]
            at + 8 <= usize::MAX, // [verus]
            m & 0x7f7f_7f7f_7f7f_7f7f == 0, // [verus]
            forall|j: u64| j < 8 ==> (#[trigger] marked(m, j) <==> (j >= p && marked(m0, j))), // [verus]
            starts@ == old(starts)@ + marks(m0, at as int, 0, p), // [verus]
        decreases m, // [verus]
    {
        /* The next line starts past the newline. */
        let ghost t = m.trailing_zeros() as u64; // [verus]
        let ghost j: u64 = t / 8; // [verus]
        proof { step(m, m0, t, j, p, at as int); } // [verus]
        // [canary]
        starts.falloc_push(at + (m.trailing_zeros() / 8) as usize + 1).ok()?;
        proof { // [verus]
            assert(marks(m0, at as int, 0, j + 1) == marks(m0, at as int, 0, p).push((at + j + 1) as usize)); // [verus]
            clear_lowest(m, t); // [verus]
            shrinks(m); // [verus]
            p = j + 1; // [verus]
        } // [verus]
        m &= m - 1;
    }
    proof { // [verus]
        assert forall|j: u64| j < 8 && j >= p implies !#[trigger] marked(m0, j) by { // [verus]
            assert(!marked(m, j)) by { zero_unmarked(m, j); } // [verus]
        } // [verus]
        no_marks_from(m0, at as int, p, 8); // [verus]
    } // [verus]
    Some(())
}

/// The word `r` built from `b0..b7` holds them, first lowest.
proof fn word_bytes(b0: u8, b1: u8, b2: u8, b3: u8, b4: u8, b5: u8, b6: u8, b7: u8, r: u64)
    requires
        r == (b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24
            | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56,
    ensures
        forall|j: u64| j < 8 ==> #[trigger] byte(r, j) == (if j == 0 { b0 } else if j == 1 { b1 }
            else if j == 2 { b2 } else if j == 3 { b3 } else if j == 4 { b4 } else if j == 5 { b5 }
            else if j == 6 { b6 } else { b7 }) as u64,
{
    assert((r >> 0u64) & 0xff == b0 as u64 && (r >> 8u64) & 0xff == b1 as u64
        && (r >> 16u64) & 0xff == b2 as u64 && (r >> 24u64) & 0xff == b3 as u64
        && (r >> 32u64) & 0xff == b4 as u64 && (r >> 40u64) & 0xff == b5 as u64
        && (r >> 48u64) & 0xff == b6 as u64 && (r >> 56u64) & 0xff == b7 as u64) by (bit_vector)
        requires
            r == (b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24
                | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56;
}

/// The mask is exact: byte `j`'s high bit is set exactly when byte `j` is
/// `\n`. Adding `0x7f` to a byte's low seven bits carries into its high bit
/// unless they are all zero, and never out of the byte.
proof fn mask_exact(w: u64, j: u64)
    by (bit_vector)
    requires
        j < 8,
    ensures
        (!(((w ^ 0x0a0a_0a0a_0a0a_0a0au64) & 0x7f7f_7f7f_7f7f_7f7fu64).wrapping_add(
            0x7f7f_7f7f_7f7f_7f7fu64,
        ) | (w ^ 0x0a0a_0a0a_0a0a_0a0au64) | 0x7f7f_7f7f_7f7f_7f7fu64) >> (8 * j + 7) as u64) & 1
            == 1 <==> (w >> (8 * j) as u64) & 0xff == 0x0a,
{
}

/// And it sets no other bit.
proof fn mask_high_only(w: u64)
    by (bit_vector)
    ensures
        !(((w ^ 0x0a0a_0a0a_0a0a_0a0au64) & 0x7f7f_7f7f_7f7f_7f7fu64).wrapping_add(
            0x7f7f_7f7f_7f7f_7f7fu64,
        ) | (w ^ 0x0a0a_0a0a_0a0a_0a0au64) | 0x7f7f_7f7f_7f7f_7f7fu64) & 0x7f7f_7f7f_7f7f_7f7fu64
            == 0,
{
}

/// No bit of their union: no bit of either.
proof fn both_clear(a: u64, b: u64)
    by (bit_vector)
    requires
        a | b == 0,
    ensures
        a == 0 && b == 0,
{
}

proof fn zero_unmarked(m: u64, j: u64)
    by (bit_vector)
    requires
        m == 0,
    ensures
        (m >> (8 * j + 7) as u64) & 1 != 1,
{
}

/// The lowest set bit of a mask with only high bits is one: `t = 8 j + 7`.
proof fn lowest_is_high(m: u64, t: u64)
    by (bit_vector)
    requires
        t < 64,
        (m >> t) & 1 == 1,
        m & 0x7f7f_7f7f_7f7f_7f7f == 0,
    ensures
        t % 8 == 7,
{
}

/// `m & (m - 1)` clears the lowest set bit `t` and keeps every other.
proof fn clear_bit(m: u64, t: u64, i: u64)
    by (bit_vector)
    requires
        t < 64,
        i < 64,
        (m >> t) & 1 == 1,
        m << (64 - t) as u64 == 0,
    ensures
        ((m & sub(m, 1)) >> i) & 1 == (if i == t { 0 } else { (m >> i) & 1 }),
{
}

/// Clearing the lowest set bit makes the word smaller: the loop ends.
proof fn shrinks(m: u64)
    by (bit_vector)
    requires
        m != 0,
    ensures
        m & sub(m, 1) < m,
{
}

proof fn subset_low7(m: u64)
    by (bit_vector)
    requires
        m & 0x7f7f_7f7f_7f7f_7f7f == 0,
    ensures
        (m & sub(m, 1)) & 0x7f7f_7f7f_7f7f_7f7f == 0,
{
}

/// One turn of `push_starts`' loop: the lowest set bit `t` is byte `j`'s
/// high bit, `j` is at or past `p`, and the bytes `p..j` are unmarked - so
/// `marks` up to `j + 1` is `marks` up to `p` and the one offset pushed.
proof fn step(m: u64, m0: u64, t: u64, j: u64, p: int, at: int)
    requires
        m != 0,
        0 <= p <= 8,
        t == u64_trailing_zeros(m),
        j == t / 8,
        m & 0x7f7f_7f7f_7f7f_7f7f == 0,
        forall|jj: u64| jj < 8 ==> (#[trigger] marked(m, jj) <==> (jj >= p && marked(m0, jj))),
    ensures
        t == 8 * j + 7,
        p <= j < 8,
        marked(m, j),
        marked(m0, j),
        marks(m0, at, 0, j + 1) == marks(m0, at, 0, p).push((at + j + 1) as usize),
{
    lowest_is_high(m, t);
    assert(marked(m, j));
    assert forall|jj: u64| p <= jj < j implies !#[trigger] marked(m0, jj) by {
        assert((m >> (8 * jj + 7) as u64) & 1u64 == 0u64);
        assert(!marked(m, jj));
    }
    no_marks_from(m0, at, p, j as int);
}

/// After `m &= m - 1`: byte `j`'s mark gone, the others kept.
proof fn clear_lowest(m: u64, t: u64)
    requires
        m != 0,
        t == u64_trailing_zeros(m),
        m & 0x7f7f_7f7f_7f7f_7f7f == 0,
    ensures
        forall|jj: u64| jj < 8 ==> (#[trigger] marked((m & sub(m, 1)) as u64, jj) <==> (8 * jj + 7
            != t && marked(m, jj))),
        (m & sub(m, 1)) & 0x7f7f_7f7f_7f7f_7f7f == 0,
{
    assert forall|jj: u64| jj < 8 implies (#[trigger] marked((m & sub(m, 1)) as u64, jj) <==> (8
        * jj + 7 != t && marked(m, jj))) by {
        clear_bit(m, t, (8 * jj + 7) as u64);
    }
    subset_low7(m);
}

/// With no byte of `lo..hi` marked, `marks` up to `hi` is `marks` up to `lo`
/// (from `base`).
proof fn no_marks_from(m: u64, at: int, lo: int, hi: int)
    requires
        0 <= lo <= hi <= 8,
        forall|jj: u64| lo <= jj < hi ==> !#[trigger] marked(m, jj),
    ensures
        marks(m, at, 0, hi) == marks(m, at, 0, lo),
    decreases hi - lo,
{
    if hi > lo {
        assert(!marked(m, (hi - 1) as u64));
        no_marks_from(m, at, lo, hi - 1);
    }
}

/// A word with no mark adds nothing.
proof fn no_marks(m: u64, at: int, lo: int, hi: int)
    requires
        m == 0,
        0 <= lo <= hi <= 8,
    ensures
        marks(m, at, lo, hi) == Seq::<usize>::empty(),
    decreases hi - lo,
{
    if hi > lo {
        zero_unmarked(m, (hi - 1) as u64);
        no_marks(m, at, lo, hi - 1);
    }
}

/// The word at `at` with its newlines marked in `m`: the table past its
/// first `k` bytes is the table before it and `marks` of those.
proof fn word_marks_k(s: Seq<u8>, at: int, m: u64, k: int)
    requires
        0 <= at,
        0 <= k <= 8,
        at + 8 <= s.len(),
        forall|j: u64| j < 8 ==> (#[trigger] marked(m, j) <==> s[at + j] == 0x0a),
    ensures
        starts_of(s, at + k) == starts_of(s, at) + marks(m, at, 0, k),
    decreases k,
{
    if k == 0 {
        assert(starts_of(s, at) + marks(m, at, 0, 0) =~= starts_of(s, at));
    } else {
        word_marks_k(s, at, m, k - 1);
        assert(marked(m, (k - 1) as u64) <==> s[at + k - 1] == 0x0a);
        if marked(m, (k - 1) as u64) {
            assert(starts_of(s, at) + marks(m, at, 0, k) =~= (starts_of(s, at) + marks(m, at, 0, k - 1)).push((at + k) as usize));
        }
    }
}

proof fn word_marks(s: Seq<u8>, at: int, m: u64)
    requires
        0 <= at,
        at + 8 <= s.len(),
        forall|j: u64| j < 8 ==> (#[trigger] marked(m, j) <==> s[at + j] == 0x0a),
    ensures
        starts_of(s, at + 8) == starts_of(s, at) + marks(m, at, 0, 8),
{
    word_marks_k(s, at, m, 8);
}

} // verus!

fn main() {}
