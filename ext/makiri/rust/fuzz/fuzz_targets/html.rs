//! The HTML parser + compat index build, on raw bytes.
//!
//! This is the cargo-fuzz counterpart to the XML reader target: it drives
//! `mkr_parse_html` and the lazy `dom_index` build, reaching the Lexbor
//! pipeline, the UTF-8 sanitizer, the attr->owner / tag->element index, and
//! the source-location recorder. Arbitrary bytes are valid input: invalid
//! UTF-8 is replaced and NUL is left for the tokenizer, so there is no
//! in-contract filter.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::dom_adapter::dom_index::mkr_parsed_dom_index_build;
use makiri::dom_adapter::post_parse::{mkr_parse_html, mkr_parsed_destroy};

fuzz_target!(|data: &[u8]| {
    unsafe {
        let p = mkr_parse_html(data.as_ptr(), data.len(), false);
        if p.is_null() {
            return;
        }

        // Force the lazy attr->owner and tag CSR index to build. This is where
        // most of the compat-layer allocation happens, so it is the memory-safety
        // surface we want under the fuzzer.
        let _ = mkr_parsed_dom_index_build(p);

        mkr_parsed_destroy(p);
    }
});
