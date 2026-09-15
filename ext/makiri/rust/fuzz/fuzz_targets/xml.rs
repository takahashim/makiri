//! The XML reader, on raw bytes (was ext/makiri/fuzz/xml_fuzz.c).
//!
//! No filtering: the parser's contract says "valid UTF-8, NUL-free", but it is
//! fail-closed and validates as it goes, so feeding it arbitrary bytes is what
//! reaches the invalid-UTF-8 and unexpected-NUL error paths. A partial document
//! must never come back - only a document or a status.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;

fuzz_target!(|data: &[u8]| {
    // The document, if any, is dropped here.
    let _ = mkr_xml_parse(data);
});
