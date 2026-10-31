//! Verus proofs over `engine/src/xpath/number.rs`'s `extent`: the grammar gate
//! the XPath lexer (`xpath::lex`) and `number()` (`xpath::value`) read a Number
//! through, for inputs of every length (the Kani harnesses it replaced stopped at 6 bytes).
//!
//! The production is `Digits ('.' Digits?)? | '.' Digits` (XPath 1.0 [30],
//! [31]); `is_number(s, n)` below is it, written from the grammar rather than
//! from `extent`. Proved over the shipped code (`extent` and `digits_end` are
//! copied, and `rake verus:sync` checks them against the source):
//!
//! - `extent(s)` stays inside `s`, and is either 0 or the length of a prefix
//!   that IS a Number;
//! - no longer prefix is a Number (maximal munch: a shorter answer would leave
//!   the lexer to restart mid-number), and 0 means no prefix is.
//!
//! What `from_extent` then hands `f64::from_str` follows: digits and at most
//! one '.', all ASCII, no sign and no exponent - a strict subset of what the
//! standard parser accepts, so the conversion cannot fail. The parser itself
//! is std's and not proved.
//!
//! Nothing is assumed: no `external_body`, no `assume_specification`.

use vstd::prelude::*;

verus! {

pub open spec fn is_digit(b: u8) -> bool {
    0x30 <= b && b <= 0x39
}

/// `s[a..b]` is all digits.
pub open spec fn digits(s: Seq<u8>, a: int, b: int) -> bool {
    forall|i: int| a <= i < b ==> is_digit(#[trigger] s[i])
}

/// `s[..n]` is a Number: `Digits`, `Digits '.'`, `Digits '.' Digits` or
/// `'.' Digits`.
pub open spec fn is_number(s: Seq<u8>, n: int) -> bool {
    0 < n <= s.len() && (digits(s, 0, n) || exists|p: int|
        0 <= p < n && #[trigger] s[p] == 0x2e && digits(s, 0, p) && digits(s, p + 1, n) && n >= 2)
}

pub fn extent(s: &[u8]) -> (r: usize)
    ensures // [verus]
        r <= s.len(), // [verus]
        r == 0 || is_number(s@, r as int), // [verus]
        forall|k: int| r < k <= s.len() ==> !is_number(s@, k), // [verus]
{
    let int_end = digits_end(s, 0);
    proof { no_longer_digits(s@, int_end as int); } // [verus]
    if int_end < s.len() && s[int_end] == b'.' {
        let end = digits_end(s, int_end + 1);
        proof { past_the_point(s@, int_end as int, end as int); } // [verus]
        if end == 1 {
            // [canary]
            0
        } else {
            // [canary]
            end
        }
    } else {
        proof { no_point_after(s@, int_end as int); } // [verus]
        // [canary]
        int_end
    }
}

fn digits_end(s: &[u8], from: usize) -> (r: usize)
    requires // [verus]
        from <= s.len(), // [verus]
    ensures // [verus]
        from <= r <= s.len(), // [verus]
        digits(s@, from as int, r as int), // [verus]
        r == s.len() || !is_digit(s@[r as int]), // [verus]
{
    let mut i = from;
    while i < s.len() && s[i] >= b'0' && s[i] <= b'9'
        invariant // [verus]
            from <= i <= s.len(), // [verus]
            digits(s@, from as int, i as int), // [verus]
        decreases s.len() - i, // [verus]
    {
        i += 1;
    }
    i
}

/// A Number's first '.' is where its leading digits end: a prefix of only
/// digits, or a '.' at `p`, has digits up to `p`, and `w` is where the
/// digits from 0 stop.
proof fn point_is_at_int(s: Seq<u8>, w: int, n: int, p: int)
    requires
        0 <= w <= s.len(),
        digits(s, 0, w),
        w == s.len() || !is_digit(s[w]),
        0 <= p < n <= s.len(),
        s[p] == 0x2e,
        digits(s, 0, p),
    ensures
        p == w,
{
    if p < w {
        assert(is_digit(s[p]));
    } else if p > w {
        assert(is_digit(s[w]));
    }
}

/// No Number is longer than `w` digits, unless a '.' follows them: a
/// longer all-digit prefix would reach `s[w]`, and a '.' inside one would
/// have to sit at `w`.
proof fn no_longer_digits(s: Seq<u8>, w: int)
    requires
        0 <= w <= s.len(),
        digits(s, 0, w),
        w == s.len() || !is_digit(s[w]),
    ensures
        forall|k: int| w < k <= s.len() && digits(s, 0, k) ==> false,
        forall|k: int, p: int|
            #![trigger s[p], is_number(s, k)]
            w < k <= s.len() && 0 <= p < k && s[p] == 0x2e && digits(s, 0, p) ==> p == w,
{
    assert forall|k: int| w < k <= s.len() && digits(s, 0, k) implies false by {
        assert(is_digit(s[w]));
    }
    assert forall|k: int, p: int|
        #![trigger s[p], is_number(s, k)]
        w < k <= s.len() && 0 <= p < k && s[p] == 0x2e && digits(s, 0, p) implies p == w by {
        point_is_at_int(s, w, k, p);
    }
}

/// No '.' at `w`: then no prefix longer than `w` is a Number.
proof fn no_point_after(s: Seq<u8>, w: int)
    requires
        0 <= w <= s.len(),
        digits(s, 0, w),
        w == s.len() || !is_digit(s[w]),
        w == s.len() || s[w] != 0x2e,
        forall|k: int| w < k <= s.len() && digits(s, 0, k) ==> false,
        forall|k: int, p: int|
            #![trigger s[p], is_number(s, k)]
            w < k <= s.len() && 0 <= p < k && s[p] == 0x2e && digits(s, 0, p) ==> p == w,
    ensures
        w == 0 || is_number(s, w),
        forall|k: int| w < k <= s.len() ==> !is_number(s, k),
{
    assert forall|k: int| w < k <= s.len() implies !is_number(s, k) by {
        if is_number(s, k) {
            if !digits(s, 0, k) {
                let p = choose|p: int|
                    0 <= p < k && #[trigger] s[p] == 0x2e && digits(s, 0, p) && digits(s, p + 1, k) && k >= 2;
                assert(p == w);
            }
        }
    }
}

/// A '.' at `w`, digits after it to `end`: `s[..end]` is a Number unless
/// it is the '.' alone, and nothing longer is one.
proof fn past_the_point(s: Seq<u8>, w: int, end: int)
    requires
        0 <= w < s.len(),
        s[w] == 0x2e,
        digits(s, 0, w),
        w + 1 <= end <= s.len(),
        digits(s, w + 1, end),
        end == s.len() || !is_digit(s[end]),
        forall|k: int| w < k <= s.len() && digits(s, 0, k) ==> false,
        forall|k: int, p: int|
            #![trigger s[p], is_number(s, k)]
            w < k <= s.len() && 0 <= p < k && s[p] == 0x2e && digits(s, 0, p) ==> p == w,
    ensures
        end != 1 ==> is_number(s, end),
        end == 1 ==> forall|k: int| 0 < k <= s.len() ==> !is_number(s, k),
        forall|k: int| end < k <= s.len() ==> !is_number(s, k),
{
    if end != 1 {
        assert(is_number(s, end)) by {
            assert(0 <= w < end && s[w] == 0x2e && digits(s, 0, w) && digits(s, w + 1, end));
        }
    }
    assert forall|k: int| end < k <= s.len() implies !is_number(s, k) by {
        if is_number(s, k) {
            if !digits(s, 0, k) {
                let p = choose|p: int|
                    0 <= p < k && #[trigger] s[p] == 0x2e && digits(s, 0, p) && digits(s, p + 1, k) && k >= 2;
                assert(p == w);
                assert(is_digit(s[end]));
            } else {
                assert(is_digit(s[w]));
            }
        }
    }
    if end == 1 {
        assert forall|k: int| 0 < k <= s.len() implies !is_number(s, k) by {
            if is_number(s, k) && k <= 1 {
                // k == 1: the '.' alone - not digits (s[0] is the '.'), and
                // a '.' at p < 1 leaves n >= 2 unmet.
                assert(!is_digit(s[0]));
            }
        }
    }
}

} // verus!

fn main() {}
