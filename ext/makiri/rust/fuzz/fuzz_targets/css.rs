//! CSS selectors over ONE fixed XML document.
//!
//! `Makiri::XML` answers `css` by lowering the selector to an XPath AST and
//! evaluating that, so this reaches Lexbor's selector parser, the lowering in
//! `css/`, and the evaluator on the shapes only the lowering builds (the
//! of-type position hooks, class and attribute matching, combinators). The
//! first input byte picks whether a bare type selector binds the document's
//! default namespace - the two ways the glue calls the lowering - and the rest
//! is the selector.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::css::{compile_owned, CssNs};

const FIXED_XML: &[u8] = b"<?xml version='1.0'?>\
<root xmlns='http://example.com/default' xmlns:ns='http://example.com/ns'>\
  <a id='x' class='c1 c2' lang='en-US' title='t'>text1</a>\
  <b id='y'><c/><c class='c1'/><d/><c/>tail</b>\
  <ns:e ns:attr='v'><c/>namespaced</ns:e>\
  <!-- comment -->\
  <?pi target='value'?>\
</root>";

fuzz_target!(|data: &[u8]| {
    let Some((&mode, selector)) = data.split_first() else {
        return;
    };
    let Ok(mut doc) = xml_parse(FIXED_XML) else {
        return;
    };
    let Some(expr) = Expr::new(selector) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };

    unsafe {
        let Some(ctx) = xml_context(&mut doc) else {
            return;
        };
        let l = limits(ctx.as_ptr());
        l.max_eval_ops = 1_000_000;
        l.max_nodeset_size = 10_000;
        l.max_string_bytes = 64 * 1024;

        // What `build_ctx` registers from the caller's namespace hash: a
        // prefix, and - when the hash names one - the default namespace under
        // the sentinel prefix the lowering then binds bare type selectors to.
        let register = |prefix: &[u8], uri: &[u8]| {
            if let (Some(p), Some(u)) = (
                VerifiedText::from_bytes(prefix),
                VerifiedText::from_bytes(uri),
            ) {
                xpath_register_ns(ctx.as_ptr(), p, u);
            }
        };
        register(b"ns", b"http://example.com/ns");
        let ns = if mode & 1 == 0 {
            CssNs {
                default_prefix: core::ptr::null(),
            }
        } else {
            register(b"xmlns", b"http://example.com/default");
            CssNs {
                default_prefix: c"xmlns".as_ptr(),
            }
        };

        let budget = ctx_budget(ctx.as_ptr());
        (*budget).limits.ast_nodes = 0;
        let Ok(ast) = compile_owned(text, &ns, budget) else {
            return;
        };
        evaluate_both(&ctx, &ast);
    }
});
