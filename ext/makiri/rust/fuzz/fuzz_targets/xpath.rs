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
    let Ok(mut doc) = mkr_xml_parse(FIXED_XML) else {
        return;
    };
    let Some(expr) = Expr::new(data) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };

    unsafe {
        // Compile-time budgets, tightened so a hostile expression fails fast
        // instead of burning fuzzer time on a pathological AST. Same numbers
        // the C harness used.
        let mut budget = Budget::new();
        budget.limits.max_ast_nodes = 10_000;
        budget.limits.max_expr_bytes = 16 * 1024;

        let ast = parse_raw(text, &mut budget);
        if ast.is_null() {
            return;
        }

        if let Some(ctx) = xml_context(&mut doc) {
            // The evaluator reads the budgets off the CONTEXT, not off a local
            // struct, so they have to be tightened through ctx_limits -
            // otherwise the evaluation runs under the large defaults and one
            // input can stall the fuzzer.
            let l = ctx_limits(ctx);
            (*l).max_eval_ops = 5_000_000;
            (*l).max_nodeset_size = 10_000;
            (*l).max_string_bytes = 1024 * 1024;
            (*l).max_recursion_depth = 64;

            let _ = evaluate(ctx, ast);
            xpath_context_free(ctx);
        }

        node_free(ast);
    }
});
