//! The XPath front end and evaluator over ONE fixed document
//! (was ext/makiri/fuzz/xpath_fuzz.c).
//!
//! The document is static so the coverage signal comes from the engine rather
//! than the reader; the input is the expression. `xml_xpath` is the
//! complementary target where the fuzzer controls both.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;

/// Enough shape to walk: two namespaces, attributes in both, an element with
/// repeated children, a comment and a PI.
const FIXED_XML: &[u8] = b"<?xml version='1.0'?>\
<root xmlns='http://example.com/default' xmlns:ns='http://example.com/ns'>\
  <a id='1' ns:attr='x'>text1</a>\
  <b id='2'><c/><c/></b>\
  <ns:d>namespaced</ns:d>\
  <!-- comment -->\
  <?pi target='value'?>\
</root>";

fuzz_target!(|data: &[u8]| {
    unsafe {
        let mut status: i32 = 0;
        let doc = mkr_xml_parse(FIXED_XML.as_ptr() as *const _, FIXED_XML.len(), &mut status);
        if doc.is_null() || (*doc).doc_node.is_null() {
            if !doc.is_null() {
                mkr_xml_doc_destroy(doc);
            }
            return;
        }

        let Some(expr) = Expr::new(data) else {
            mkr_xml_doc_destroy(doc);
            return;
        };

        // Compile-time budgets, tightened so a hostile expression fails fast
        // instead of burning fuzzer time on a pathological AST. Same numbers
        // the C harness used.
        let mut limits: Limits = core::mem::zeroed();
        mkr_xpath_limits_init_defaults(&mut limits);
        limits.max_ast_nodes = 10_000;
        limits.max_expr_bytes = 16 * 1024;

        let mut err: XPathError = core::mem::zeroed();
        let ast = mkr_parse(expr.text(), &mut limits, &mut err);
        if ast.is_null() {
            mkr_xpath_error_clear(&mut err);
            mkr_xml_doc_destroy(doc);
            return;
        }

        let ctx = mkr_xpath_context_new((*doc).doc_node as *mut _, (*doc).doc_node as *mut _);
        if !ctx.is_null() {
            mkr_xpath_set_engine_kind(ctx, ENGINE_XML);
            // The evaluator reads the budgets off the CONTEXT, not off a local
            // struct, so they have to be tightened through mkr_ctx_limits -
            // otherwise the evaluation runs under the large defaults and one
            // input can stall the fuzzer.
            let l = mkr_ctx_limits(ctx);
            (*l).max_eval_ops = 5_000_000;
            (*l).max_nodeset_size = 10_000;
            (*l).max_string_bytes = 1024 * 1024;
            (*l).max_recursion_depth = 64;

            let mut out: XPathValue = core::mem::zeroed();
            let mut eval_err: XPathError = core::mem::zeroed();
            if mkr_xpath_eval_compiled(ctx, ast, &mut out, &mut eval_err) == 0 {
                mkr_xpath_value_clear(&mut out);
            } else {
                mkr_xpath_error_clear(&mut eval_err);
            }
            mkr_xpath_context_free(ctx);
        }

        mkr_node_free(ast);
        mkr_xml_doc_destroy(doc);
    }
});
