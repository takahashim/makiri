//! `decode1` against `core::str::from_utf8`, on every input it can be asked
//! about.
//!
//! `verus/utf8.rs` proves `decode1` answers exactly vstd's statement of UTF-8,
//! at every length. That statement is not the validator the rest of the crate
//! calls (`valid`, which is `from_utf8`), and Verus has no contract for
//! `from_utf8` to prove against. This ties the two by running them: a code
//! point is at most four bytes, and `decode1` reads no further, so every
//! buffer of up to four bytes is every input there is to compare on.
//!
//! It was a Kani harness, and running every input is the same check - 4.3
//! billion buffers, about a minute on one core in release - with a stronger
//! claim: Kani checked that an accepted prefix is valid and that valid input
//! decodes, this that `decode1`'s answer IS the first character `from_utf8`
//! reads, on every input, accepted or not. `rake rust:exhaustive` runs it in
//! release (CI: verify.yml); a debug build would take far longer, so a plain
//! `cargo test` leaves it out.

use super::decode1;

/// What `from_utf8` reads first: the first character of the longest valid
/// prefix and its length, or `None` when no prefix is valid.
fn std_first(s: &[u8]) -> Option<(u32, usize)> {
    let valid = match core::str::from_utf8(s) {
        Ok(t) => t,
        Err(e) => core::str::from_utf8(s.get(..e.valid_up_to())?).ok()?,
    };
    let c = valid.chars().next()?;
    Some((c as u32, c.len_utf8()))
}

/// Every buffer whose first byte is `b0`: each of the 2^24 tails at length
/// four, and at the shorter lengths only the tails whose unused bytes are
/// zero, so that no buffer is checked twice.
fn check_lead(b0: u8) {
    for rest in 0..=0x00FF_FFFFu32 {
        let [b1, b2, b3, _] = rest.to_le_bytes();
        let buf = [b0, b1, b2, b3];
        for len in 1..=4usize {
            if len < 4 && rest >> (8 * (len - 1)) != 0 {
                continue;
            }
            let s = &buf[..len];
            assert_eq!(
                decode1(s),
                std_first(s),
                "decode1 and from_utf8 differ on {s:02x?}"
            );
        }
    }
}

#[test]
#[ignore = "4.3 billion inputs: `rake rust:exhaustive` runs it in release"]
fn decode1_agrees_with_from_utf8_on_every_input() {
    assert_eq!(decode1(&[]), std_first(&[]));
    std::thread::scope(|scope| {
        for b0 in 0..=255u8 {
            scope.spawn(move || check_lead(b0));
        }
    });
}
