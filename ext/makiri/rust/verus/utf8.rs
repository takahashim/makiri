//! Verus proofs over the strict UTF-8 decoder (`engine/src/utf8_decode.rs`'s
//! `decode1`) and the XML Char scan built on it (`engine/src/xml/chars/mod.rs`'s
//! `all_code_points` and `validate_chars`), for inputs of every length - the
//! Kani harnesses they replaced stopped at 8 bytes.
//!
//! What they are held to is `vstd::utf8`: Verus's own statement of UTF-8 -
//! `valid_first_scalar` (the bit patterns, no overlong form, no surrogate),
//! `decode_first_scalar`, `valid_utf8` (a sequence of such scalars) - written
//! by others and not from this decoder, which is what makes agreement
//! evidence rather than a restatement. Proved over the shipped code (each
//! function is copied, and `rake verus:sync` checks it against the source):
//!
//! - `decode1(p)` answers exactly when `p` begins with a well-formed scalar,
//!   with that scalar and its length;
//! - `all_code_points` accepts only valid UTF-8, and with a predicate that
//!   accepts every code point, accepts exactly valid UTF-8: the chain of
//!   decodes consumes valid input exactly and nothing else;
//! - so `validate_chars` accepts only valid UTF-8, whatever its length.
//!
//! What is not proved here: that `decode1` agrees with `core::str::from_utf8`,
//! std's validator, which Verus has no contract for - the tie between vstd's
//! model and the validator the rest of the crate calls. `utf8_decode`'s
//! exhaustive test runs that on every buffer of up to four bytes
//! (`rake rust:exhaustive`).
//!
//! Nothing is assumed: no `external_body`, no `assume_specification` (the
//! source spells out the scalar test rather than calling `char::from_u32`,
//! whose contract vstd does not state).

use vstd::prelude::*;
use vstd::utf8::*;

verus! {

broadcast use vstd::seq_lib::group_seq_properties;

/// The decoder's continuation loop: `init` with the six data bits of each of
/// `tail[..i]` shifted in, as the loop computes it.
pub open spec fn acc(init: u32, tail: Seq<u8>, i: int) -> u32
    decreases i,
{
    if i <= 0 {
        init
    } else {
        ((acc(init, tail, i - 1) << 6u32) | ((tail[i - 1] as u32) & 0x3Fu32)) as u32
    }
}

proof fn ascii(b: u8)
    by (bit_vector)
    requires
        (b as u32) < 0x80,
    ensures
        is_leading_byte_width_1(b),
        codepoint_width_1(b) == b as u32,
{
}

proof fn lead_patterns(b: u8)
    by (bit_vector)
    ensures
        (b as u32) & 0xE0 == 0xC0 <==> is_leading_byte_width_2(b),
        (b as u32) & 0xF0 == 0xE0 <==> is_leading_byte_width_3(b),
        (b as u32) & 0xF8 == 0xF0 <==> is_leading_byte_width_4(b),
        b & 0xC0 != 0x80 <==> !is_continuation_byte(b),
{
}

proof fn width_2(b1: u8, b2: u8)
    by (bit_vector)
    requires
        is_leading_byte_width_2(b1),
        is_continuation_byte(b2),
    ensures
        ((((b1 as u32) & 0x1F) << 6u32) | ((b2 as u32) & 0x3F)) == codepoint_width_2(b1, b2),
        codepoint_width_2(b1, b2) < 0x800,
{
}

proof fn width_3(b1: u8, b2: u8, b3: u8)
    by (bit_vector)
    requires
        is_leading_byte_width_3(b1),
        is_continuation_byte(b2),
        is_continuation_byte(b3),
    ensures
        ((((((b1 as u32) & 0x0F) << 6u32) | ((b2 as u32) & 0x3F)) << 6u32) | ((b3 as u32) & 0x3F))
            == codepoint_width_3(b1, b2, b3),
        codepoint_width_3(b1, b2, b3) < 0x10000,
{
}

proof fn width_4(b1: u8, b2: u8, b3: u8, b4: u8)
    by (bit_vector)
    requires
        is_leading_byte_width_4(b1),
        is_continuation_byte(b2),
        is_continuation_byte(b3),
        is_continuation_byte(b4),
    ensures
        ((((((((b1 as u32) & 0x07) << 6u32) | ((b2 as u32) & 0x3F)) << 6u32) | ((b3 as u32)
            & 0x3F)) << 6u32) | ((b4 as u32) & 0x3F)) == codepoint_width_4(b1, b2, b3, b4),
{
}

pub fn decode1(p: &[u8]) -> (r: Option<(u32, usize)>)
    ensures // [verus]
        r.is_some() <==> valid_first_scalar(p@), // [verus]
        r.is_some() ==> r.unwrap().0 == decode_first_scalar(p@) // [verus]
            && r.unwrap().1 as int == length_of_first_scalar(p@), // [verus]
{
    let b0 = *p.first()? as u32;
    if b0 < 0x80 {
        proof { ascii(p@[0]); } // [verus]
        // [canary]
        return Some((b0, 1));
    }
    decode_multibyte(p, b0)
}

fn decode_multibyte(p: &[u8], b0: u32) -> (r: Option<(u32, usize)>)
    requires // [verus]
        p.len() >= 1, // [verus]
        b0 == p@[0] as u32, // [verus]
        b0 >= 0x80, // [verus]
    ensures // [verus]
        r.is_some() <==> valid_first_scalar(p@), // [verus]
        r.is_some() ==> r.unwrap().0 == decode_first_scalar(p@) // [verus]
            && r.unwrap().1 as int == length_of_first_scalar(p@), // [verus]
{
    proof { lead_patterns(p@[0]); } // [verus]
    let (len, min, init) = if b0 & 0xE0 == 0xC0 {
        (2usize, 0x80u32, b0 & 0x1F)
    } else if b0 & 0xF0 == 0xE0 {
        (3, 0x800, b0 & 0x0F)
    } else if b0 & 0xF8 == 0xF0 {
        (4, 0x10000, b0 & 0x07)
    } else {
        /* A continuation byte or an 0xF8+ lead: never starts a code point. */
        // [canary]
        return None;
    };
    let tail = p.get(1..len)?; /* None when truncated */
    let mut cp = init;
    let mut i = 0;
    while i < tail.len()
        invariant // [verus]
            2 <= len <= 4, // [verus]
            len <= p.len(), // [verus]
            len == 2 ==> is_leading_byte_width_2(p@[0]), // [verus]
            len == 3 ==> is_leading_byte_width_3(p@[0]), // [verus]
            len == 4 ==> is_leading_byte_width_4(p@[0]), // [verus]
            min == if len == 2 { 0x80u32 } else if len == 3 { 0x800u32 } else { 0x10000u32 }, // [verus]
            init == if len == 2 { b0 & 0x1F } else if len == 3 { b0 & 0x0F } else { b0 & 0x07 }, // [verus]
            tail@ == p@.subrange(1, len as int), // [verus]
            0 <= i <= tail.len(), // [verus]
            cp == acc(init, tail@, i as int), // [verus]
            forall|j: int| 0 <= j < i ==> is_continuation_byte(#[trigger] tail@[j]), // [verus]
        decreases tail.len() - i, // [verus]
    {
        let b = tail[i];
        proof { lead_patterns(b); } // [verus]
        if b & 0xC0 != 0x80 {
            // [canary]
            return None;
        }
        cp = (cp << 6) | (b as u32 & 0x3F);
        i += 1;
    }
    proof { // [verus]
        reveal_with_fuel(acc, 4); // [verus]
        if len == 2 { width_2(p@[0], p@[1]); } // [verus]
        else if len == 3 { width_3(p@[0], p@[1], p@[2]); } // [verus]
        else { width_4(p@[0], p@[1], p@[2], p@[3]); } // [verus]
    } // [verus]
    /* Overlong, past U+10FFFF, or a surrogate: not a scalar value. */
    if cp < min || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
        // [canary]
        return None;
    }
    // [canary]
    Some((cp, len))
}

/// XML 1.0 §2.2 Char, copied for `validate_chars`. What it answers does not
/// matter to the proof: `all_code_points` accepts only valid UTF-8 whatever
/// its predicate says.
pub fn is_char(c: u32) -> bool {
    c == 0x9
        || c == 0xA
        || c == 0xD
        || (0x20..=0xD7FF).contains(&c)
        || (0xE000..=0xFFFD).contains(&c)
        || (0x10000..=0x10FFFF).contains(&c)
}

fn all_code_points(s: &[u8], ok: impl Fn(u32) -> bool) -> (r: bool)
    requires // [verus]
        forall|c: u32| ok.requires((c,)), // [verus]
    ensures // [verus]
        r ==> valid_utf8(s@), // [verus]
        (forall|c: u32, b: bool| #[trigger] ok.ensures((c,), b) ==> b) // [verus]
            ==> (r <==> valid_utf8(s@)), // [verus]
{
    let mut i = 0;
    while i < s.len()
        invariant // [verus]
            forall|c: u32| ok.requires((c,)), // [verus]
            partial_valid_utf8(s@, i as int), // [verus]
        decreases s.len() - i, // [verus]
    {
        proof { // [verus]
            let rest = s@.subrange(i as int, s@.len() as int); // [verus]
            if valid_first_scalar(rest) { // [verus]
                partial_valid_utf8_extend(s@, i as int); // [verus]
                // [canary]
            } else { // [verus]
                partial_valid_partial_invalid_utf8(s@, i as int); // [verus]
                // [canary]
            } // [verus]
        } // [verus]
        match decode1(&s[i..]) {
            Some((cp, bl)) if ok(cp) => i += bl,
            _ => return false,
        }
    }
    proof { assert(s@.subrange(0, s@.len() as int) =~= s@); } // [verus]
    // [canary]
    true
}

/// All of `s` is XML Char (no reference recognition); true for empty `s`.
pub fn validate_chars(s: &[u8]) -> (r: bool)
    ensures // [verus]
        r ==> valid_utf8(s@), // [verus]
{
    all_code_points(s, is_char)
}

} // verus!

fn main() {}
