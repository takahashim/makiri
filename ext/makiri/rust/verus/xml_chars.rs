//! Verus proofs over `engine/src/xml/chars/mod.rs`'s character classes:
//! NameStartChar is a subset of NameChar, and NameChar of Char (XML 1.0 §2.2,
//! §2.3), for every `u32` - what Kani's `predicate_inclusions` proved, and
//! the last thing it proved.
//!
//! Proved over the shipped code (`is_char`, `is_name_start` and
//! `is_name_char` are copied, and `rake verus:sync` checks them against the
//! source). Each function's `ensures` states its class as the XML
//! recommendation's productions read, comparison by comparison; the
//! inclusions are then proved of what the functions answer.
//!
//! Nothing is assumed: no `external_body`, no `assume_specification`. The
//! `RangeInclusive::contains` calls are read through vstd's own
//! specification of `contains`.

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

/// XML 1.0 §2.3 [4] NameStartChar.
pub open spec fn name_start_spec(c: u32) -> bool {
    c == 0x3A || in_range(c, 0x41, 0x5A) || c == 0x5F || in_range(c, 0x61, 0x7A)
        || in_range(c, 0xC0, 0xD6) || in_range(c, 0xD8, 0xF6) || in_range(c, 0xF8, 0x2FF)
        || in_range(c, 0x370, 0x37D) || in_range(c, 0x37F, 0x1FFF) || in_range(c, 0x200C, 0x200D)
        || in_range(c, 0x2070, 0x218F) || in_range(c, 0x2C00, 0x2FEF) || in_range(c, 0x3001, 0xD7FF)
        || in_range(c, 0xF900, 0xFDCF) || in_range(c, 0xFDF0, 0xFFFD) || in_range(c, 0x10000, 0xEFFFF)
}

/// XML 1.0 §2.3 [4a] NameChar.
pub open spec fn name_char_spec(c: u32) -> bool {
    name_start_spec(c) || c == 0x2D || c == 0x2E || in_range(c, 0x30, 0x39) || c == 0xB7
        || in_range(c, 0x300, 0x36F) || in_range(c, 0x203F, 0x2040)
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

pub fn is_name_start(c: u32) -> (r: bool)
    ensures // [verus]
        r == name_start_spec(c), // [verus]
{
    c == b':' as u32
        || (b'A' as u32..=b'Z' as u32).contains(&c)
        || c == b'_' as u32
        || (b'a' as u32..=b'z' as u32).contains(&c)
        || (0xC0..=0xD6).contains(&c)
        || (0xD8..=0xF6).contains(&c)
        || (0xF8..=0x2FF).contains(&c)
        || (0x370..=0x37D).contains(&c)
        || (0x37F..=0x1FFF).contains(&c)
        || (0x200C..=0x200D).contains(&c)
        || (0x2070..=0x218F).contains(&c)
        || (0x2C00..=0x2FEF).contains(&c)
        || (0x3001..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFFD).contains(&c)
        || (0x10000..=0xEFFFF).contains(&c)
}

pub fn is_name_char(c: u32) -> (r: bool)
    ensures // [verus]
        r == name_char_spec(c), // [verus]
{
    is_name_start(c)
        || c == b'-' as u32
        || c == b'.' as u32
        || (b'0' as u32..=b'9' as u32).contains(&c)
        || c == 0xB7
        || (0x300..=0x36F).contains(&c)
        || (0x203F..=0x2040).contains(&c)
}

/// NameStartChar is a subset of NameChar, and NameChar of Char.
proof fn inclusions(c: u32)
    ensures
        name_start_spec(c) ==> name_char_spec(c),
        name_char_spec(c) ==> char_spec(c),
{
}

/// The same, of what the shipped functions answer, for every `u32`.
fn predicate_inclusions(c: u32) {
    let start = is_name_start(c);
    let name = is_name_char(c);
    let ch = is_char(c);
    proof { inclusions(c); }
    if start {
        // [canary]
        assert(name);
    }
    if name {
        // [canary]
        assert(ch);
    }
}

} // verus!

fn main() {}
