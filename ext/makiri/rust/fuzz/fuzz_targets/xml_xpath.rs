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
use makiri::xml::chars::validate_chars;

fuzz_target!(|data: &[u8]| {
    let Some(sep) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let (xml, rest) = data.split_at(sep);
    let expr_bytes = &rest[1..];

    unsafe {
        // The in-contract filter, mirroring the bridge's strict gate: this
        // target is about what the engine does with well-formed input, and the
        // `xml` target already covers the reader on arbitrary bytes. Keep the
        // XML character-class gate here so this target preserves that scope.
        if !mkr_utf8_valid(xml.as_ptr(), xml.len()) {
            return;
        }
        if !validate_chars(xml) {
            return;
        }
        if !mkr_utf8_valid(expr_bytes.as_ptr(), expr_bytes.len()) {
            return;
        }

        let mut status: i32 = 0;
        let doc = mkr_xml_parse(xml.as_ptr() as *const _, xml.len(), &mut status);
        if doc.is_null() {
            return;
        }

        let ctx = mkr_xpath_context_new((*doc).doc_node as *mut _, (*doc).doc_node as *mut _);
        if !ctx.is_null() {
            mkr_xpath_set_engine_kind(ctx, ENGINE_XML);

            // Much tighter than the `xpath` target's: here the fuzzer controls
            // the document too, so a single input could otherwise build a large
            // tree AND walk it. An overrun fails closed, which is the point.
            let l = mkr_ctx_limits(ctx);
            (*l).max_eval_ops = 20_000;
            (*l).max_nodeset_size = 1024;
            (*l).max_string_bytes = 4096;

            let d = b"d\0";
            let urn = b"urn:d\0";
            mkr_xpath_register_ns(
                ctx,
                VerifiedText {
                    ptr: d.as_ptr() as *const _,
                    len: 1,
                },
                VerifiedText {
                    ptr: urn.as_ptr() as *const _,
                    len: 5,
                },
            );

            if let Some(expr) = Expr::new(expr_bytes) {
                let mut err: XPathError = core::mem::zeroed();
                let ast = mkr_parse(expr.text(), l, &mut err);
                if !ast.is_null() {
                    let mut v: XPathValue = core::mem::zeroed();
                    let _ = mkr_xpath_eval_compiled(ctx, ast, &mut v, &mut err);
                    mkr_xpath_value_clear(&mut v);
                    mkr_node_free(ast);
                }
                mkr_xpath_error_clear(&mut err);
            }
            mkr_xpath_context_free(ctx);
        }
        mkr_xml_doc_destroy(doc);
    }
});
