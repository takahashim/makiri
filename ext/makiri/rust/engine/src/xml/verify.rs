//! Kani proofs for the XML character layer.
//!
//! These replace `verify/harness_xml_chars.c`, whose subject
//! (`xml/mkr_xml_chars.c`) the Rust build stopped compiling long before the file
//! was deleted - and `rake verify` kept passing over it the whole time, proving
//! a translation unit nothing executed. That is why the correspondence below is
//! written down rather than assumed: a proof is only evidence about the code
//! that actually runs.
//!
//! That bytes `validate_chars` accepts are UTF-8 was proved here too, up to 8
//! bytes; `verus/utf8.rs` proves it at every length, against vstd's UTF-8,
//! and so does the decoder's length contract this file also held.
//!
//! Run with `rake kani` (or `cargo kani --no-default-features`).

#![forbid(unsafe_code)]
#![cfg(kani)]

use super::chars::{is_char, is_name_char, is_name_start};

/// The predicate inclusions, over EVERY u32 - not sampled.
///
/// NameStartChar is a subset of NameChar, and NameChar of Char (XML 1.0
/// §2.2/§2.3). This is the same property the C harness proved and at the same
/// strength, since both quantify over the whole 32-bit space.
#[kani::proof]
fn predicate_inclusions() {
    let c: u32 = kani::any();
    if is_name_start(c) {
        assert!(is_name_char(c), "NameStartChar must be a NameChar");
        kani::cover!(c > 0x7F, "a non-ASCII NameStartChar");
    }
    if is_name_char(c) {
        assert!(is_char(c), "NameChar must be a Char");
        kani::cover!(!is_name_start(c), "a NameChar that cannot start a name");
    }
}
