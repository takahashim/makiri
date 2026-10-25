//! The HTML parser + compat index build, on raw bytes.
//!
//! This is the cargo-fuzz counterpart to the XML reader target: it drives a
//! document parse and the lazy `dom_index` build, reaching the ported
//! tokenizer and tree builder, the UTF-8 sanitizer, the tag -> elements
//! index, and the source-location stamps. Arbitrary bytes are valid input:
//! invalid UTF-8 is replaced and NUL is left for the tokenizer, so there is no
//! in-contract filter.
//!
//! The first byte picks the entry: the bytes as a UTF-8 String has them, or
//! as a binary one, through the encoding sniffing (`html::encoding`) - its
//! BOM check, prescan, detector, decoders and the re-read a `<meta>` makes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::html_dom::post_parse::parse_html;
use makiri::html_dom::tree_guard::ParseLimits;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, data)) = data.split_first() else {
        return;
    };
    let parsed = if mode & 1 == 0 {
        parse_html(data, false, ParseLimits::DEFAULT)
    } else {
        let input = makiri::html::encoding::DocumentInput {
            bytes: data,
            transport: None,
            known_valid_utf8: false,
        };
        makiri::html::tree::parse_document_bytes(input, ParseLimits::DEFAULT)
    };
    let Ok(p) = parsed else {
        return;
    };

    // Force the lazy tag -> elements index (a CSR) to build. This is where
    // most of the compat-layer allocation happens, so it is the memory-safety
    // surface we want under the fuzzer.
    let _ = p.ensure_dom_index();
});
