//! Fuzzer-controlled document AND expression
//! (was ext/makiri/fuzz/xml_xpath_fuzz.c).
//!
//! The other two targets each pin one half: `xml` drives the reader alone, and
//! `xpath` drives the engine over one document shape - so evaluator paths that
//! depend on document structure (namespace scopes, index buckets, axis edge
//! cases) only ever see that shape. Here the bytes up to the first NUL are the
//! document and the rest is the expression. Both engine contracts forbid an
//! embedded NUL, so spending the separator costs no coverage.
//!
//! The split rule and the in-contract filter below are the retired C harness's,
//! unchanged. That matters more than it looks: the corpus carries over as raw
//! bytes, and a corpus is only worth carrying if the same bytes still mean the
//! same document and the same expression.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::cutf8::valid;
use makiri::xml::chars::validate_chars;

fuzz_target!(|data: &[u8]| {
    let Some(sep) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let (xml, rest) = data.split_at(sep);
    let expr_bytes = &rest[1..];

    // The in-contract filter, mirroring the bridge's strict gate: this target is
    // about what the engine does with well-formed input, and the `xml` target
    // already covers the reader on arbitrary bytes. Keep the XML character-class
    // gate here so this target preserves that scope.
    if !valid(xml) || !validate_chars(xml) || !valid(expr_bytes) {
        return;
    }

    let Ok(mut doc) = mkr_xml_parse(xml) else {
        return;
    };

    unsafe {
        let Some(ctx) = xml_context(&mut doc) else {
            return;
        };

        // Much tighter than the `xpath` target's: here the fuzzer controls the
        // document too, so a single input could otherwise build a large tree
        // AND walk it. An overrun fails closed, which is the point.
        let l = ctx_limits(ctx);
        (*l).max_eval_ops = 20_000;
        (*l).max_nodeset_size = 1024;
        (*l).max_string_bytes = 4096;

        if let (Some(prefix), Some(uri)) = (
            VerifiedText::from_bytes(b"d"),
            VerifiedText::from_bytes(b"urn:d"),
        ) {
            xpath_register_ns(ctx, prefix, uri);
        }

        if let Some(text) = Expr::new(expr_bytes).as_ref().and_then(Expr::text) {
            let mut err = XPathError::new();
            let ast = parse_raw(text, l, &mut err);
            if !ast.is_null() {
                let mut v: XPathValue = core::mem::zeroed();
                let _ = xpath_eval_compiled(ctx, ast, &mut v, &mut err);
                xpath_value_clear(&mut v);
                node_free(ast);
            }
        }
        xpath_context_free(ctx);
    }
});
