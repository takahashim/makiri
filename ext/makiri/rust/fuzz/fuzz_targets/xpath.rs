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
    let Ok(doc) = xml_parse(FIXED_XML) else {
        return;
    };
    let Some(expr) = Expr::new(data) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };

    unsafe {
        let Some(mut ctx) = xml_context(&doc) else {
            return;
        };

        // Budgets tightened so a hostile expression fails fast instead of
        // burning fuzzer time. Same numbers the C harness used, and applied in
        // the same order: the compile-time pair for the parse, the rest only for
        // the evaluation.
        let l = ctx.limits_mut();
        l.max_ast_nodes = 10_000;
        l.max_expr_bytes = 16 * 1024;
        let Some(ast) = parse(&ctx, text) else {
            return;
        };

        let l = ctx.limits_mut();
        l.max_eval_ops = 5_000_000;
        l.max_nodeset_size = 10_000;
        l.max_string_bytes = 1024 * 1024;
        l.max_recursion_depth = 64;

        evaluate_both(&ctx, &ast);
    }
});
