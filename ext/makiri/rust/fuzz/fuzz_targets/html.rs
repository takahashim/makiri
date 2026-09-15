//! The HTML parser + compat index build, on raw bytes.
//!
//! This is the cargo-fuzz counterpart to the XML reader target: it drives
//! `parse_html` and the lazy `dom_index` build, reaching the Lexbor
//! pipeline, the UTF-8 sanitizer, the attr->owner / tag->element index, and
//! the source-location recorder. Arbitrary bytes are valid input: invalid
//! UTF-8 is replaced and NUL is left for the tokenizer, so there is no
//! in-contract filter.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::dom_adapter::post_parse::parse_html;

fuzz_target!(|data: &[u8]| {
    // SAFETY: `data` is a live slice for the whole call.
    let Some(mut p) = (unsafe { parse_html(data.as_ptr(), data.len(), false) }) else {
        return;
    };

    // Force the lazy attr->owner and tag CSR index to build. This is where
    // most of the compat-layer allocation happens, so it is the memory-safety
    // surface we want under the fuzzer.
    let _ = p.dom_index();
});
