//! Verus proofs over the HTML tokenizer's word-at-a-time skip
//! (`engine/src/html/tokenizer/mod.rs`'s `seek4`) and the SWAR helpers it
//! reads through (`engine/src/swar.rs`'s `word` and `has_zero`), for every
//! input - eight bytes a step is 2^64 words, past any test.
//!
//! `seek4` is how the data and attribute-value states find the next byte
//! they must stop at (`<`, `&`, `"`, CR, NUL). Its zero test is the short one,
//! whose borrow can mark a byte above a real match, so the answer rests on the
//! LOWEST marked byte being exact. Proved over the shipped code (each function
//! is copied, and `rake verus:sync` checks it against the source):
//!
//! - `word` is the eight bytes, first byte lowest;
//! - `has_zero` marks the lowest zero byte of a word (`zero_is_marked`);
//! - `seek4` skips no byte it was asked to find, and returns no position past
//!   `end` (or the input) when it starts inside it - the tokenizer's byte loop
//!   runs `while data != end`, so a skip past `end` would never stop.
//!
//! Nothing is assumed: no `external_body`, no `assume_specification`.
//! `trailing_zeros` is vstd's, which vstd proves from its definition.

use vstd::prelude::*;
use vstd::std_specs::bits::*;

verus! {

broadcast use axiom_u64_trailing_zeros;

// As `engine/src/swar.rs` defines them.
pub const ONES: u64 = u64::MAX / 0xFF;
pub const HIGH: u64 = ONES * 0x80;

/// Byte `j` of `w`, first byte lowest.
pub open spec fn byte(w: u64, j: u64) -> u64 {
    (w >> (8 * j) as u64) & 0xff
}

/// `b` is one of the four bytes `seek4` stops at.
pub open spec fn wanted(b: u8, c: [u8; 4]) -> bool {
    b == c[0] || b == c[1] || b == c[2] || b == c[3]
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

pub fn has_zero(v: u64) -> (m: u64)
    ensures // [verus]
        m == hz(v), // [verus]
{
    // [canary]
    v.wrapping_sub(ONES) & !v & HIGH
}

#[verifier::loop_isolation(false)]
pub fn seek4(input: &[u8], mut data: usize, end: usize, c: [u8; 4]) -> (r: usize)
    ensures // [verus]
        r >= data, // [verus]
        data <= end && data <= input.len() ==> r <= end && r <= input.len(), // [verus]
        forall|k: int| data <= k < r ==> !wanted(#[trigger] input@[k], c), // [verus]
{
    let ghost data0 = data; // [verus]
    let ghost end0 = end; // [verus]
    let end = end.min(input.len());
    while data <= end && end - data >= 8
        invariant // [verus]
            data0 <= data, // [verus]
            end <= input.len(), // [verus]
            end <= end0, // [verus]
            data0 <= end ==> data <= end, // [verus]
            forall|k: int| data0 <= k < data ==> !wanted(#[trigger] input@[k], c), // [verus]
        decreases end - data, // [verus]
    {
        let bytes = word(&input[data..data + 8]);
        let matches = has_zero(bytes ^ (ONES * c[0] as u64))
            | has_zero(bytes ^ (ONES * c[1] as u64))
            | has_zero(bytes ^ (ONES * c[2] as u64))
            | has_zero(bytes ^ (ONES * c[3] as u64));
        proof { none_before(input@, data as int, bytes, c, matches); } // [verus]
        if matches != 0 {
            // [canary]
            return data + (matches.trailing_zeros() / 8) as usize;
        }
        data += 8;
    }
    // [canary]
    data
}

/// `c` in every byte.
pub open spec fn spread(c: u8) -> u64 {
    (ONES * (c as u64)) as u64
}

/// `has_zero`'s answer, as a spec.
pub open spec fn hz(v: u64) -> u64 {
    v.wrapping_sub(ONES) & !v & HIGH
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

/// The heart of it: a zero byte `j` of `v` leaves a mark at or below its own
/// high bit - its own, or a lower zero byte's - so `has_zero(v)` has a bit
/// among its low `8 * j + 8`. (Shifting left by `56 - 8 * j` keeps exactly
/// those.)
proof fn zero_is_marked(v: u64, j: u64)
    by (bit_vector)
    requires
        j < 8,
        (v >> (8 * j) as u64) & 0xff == 0,
    ensures
        (v.wrapping_sub(0x0101_0101_0101_0101u64) & !v & 0x8080_8080_8080_8080u64)
            << ((56 - 8 * j) as u64) != 0,
{
}

/// A byte of `w ^ (ONES * c)` is zero exactly where `w`'s byte is `c`.
proof fn xor_finds(w: u64, c: u64, j: u64)
    by (bit_vector)
    requires
        j < 8,
        c < 256,
    ensures
        ((w ^ (0x0101_0101_0101_0101u64 * c) as u64) >> (8 * j) as u64) & 0xff == 0
            <==> (w >> (8 * j) as u64) & 0xff == c,
{
}

/// No set bit among the low `t` bits leaves none among the low `8 * j + 8`
/// when those are no more.
proof fn low_bits(m: u64, t: u64, j: u64)
    by (bit_vector)
    requires
        j < 8,
        8 * j + 8 <= t <= 64,
        m << ((64 - t) as u64) == 0,
    ensures
        m << ((56 - 8 * j) as u64) == 0,
{
}

/// A mark in any of the four is a mark in their union.
proof fn or_keeps(a: u64, b: u64, c: u64, d: u64, s: u64)
    by (bit_vector)
    requires
        a << s != 0 || b << s != 0 || c << s != 0 || d << s != 0,
    ensures
        (a | b | c | d) << s != 0,
{
}

/// None of the bytes of the word below its lowest marked byte is wanted -
/// all eight when nothing is marked.
proof fn none_before(input: Seq<u8>, data: int, w: u64, c: [u8; 4], matches: u64)
    requires
        0 <= data,
        data + 8 <= input.len(),
        forall|j: u64| j < 8 ==> #[trigger] byte(w, j) == input[data + j] as u64,
        matches == hz(w ^ spread(c[0])) | hz(w ^ spread(c[1])) | hz(w ^ spread(c[2]))
            | hz(w ^ spread(c[3])),
    ensures
        forall|k: int|
            data <= k < data + (u64_trailing_zeros(matches) / 8) as int ==> !wanted(
                #[trigger] input[k],
                c,
            ),
        matches != 0 ==> u64_trailing_zeros(matches) / 8 < 8,
{
    let t = u64_trailing_zeros(matches) as u64;
    assert forall|k: int|
        data <= k < data + (u64_trailing_zeros(matches) / 8) as int implies !wanted(
            #[trigger] input[k],
            c,
        ) by {
        let j = (k - data) as u64;
        assert(byte(w, j) == input[data + j] as u64);
        if wanted(input[k], c) {
            let ci: u8 = if input[k] == c[0] {
                c[0]
            } else if input[k] == c[1] {
                c[1]
            } else if input[k] == c[2] {
                c[2]
            } else {
                c[3]
            };
            xor_finds(w, ci as u64, j);
            zero_is_marked(w ^ spread(ci), j);
            let s = (56 - 8 * j) as u64;
            or_keeps(
                hz(w ^ spread(c[0])),
                hz(w ^ spread(c[1])),
                hz(w ^ spread(c[2])),
                hz(w ^ spread(c[3])),
                s,
            );
            low_bits(matches, t, j);
        }
    }
}

} // verus!

fn main() {}
