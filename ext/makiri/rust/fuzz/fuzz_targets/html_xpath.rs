//! XPath over a Lexbor-parsed HTML document, through the HTML engine instance.
//!
//! `html` covers the parse and the index build; `xpath` and `xml_xpath` cover
//! the evaluator over the XML instance. This is the one that reaches the HTML
//! instance - the Lexbor DOM adapter, the element-index `//tag` fast path, the
//! `[@attr]` matcher over Lexbor attributes, and strict/lax namespace matching.
//!
//! The bytes up to the first NUL are the document (arbitrary bytes are valid
//! HTML input), the next byte picks the namespace-matching mode, and the rest
//! is the expression.
#![no_main]

use core::ffi::c_void;

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::dom_adapter::dom_index::{mkr_parsed_dom_index_build, mkr_parsed_element_index};
use makiri::dom_adapter::post_parse::{
    mkr_parse_html, mkr_parsed_destroy, mkr_parsed_html_doc, Parsed,
};
use makiri::xpath::ctx::ctx_set_unprefixed_lax;

fuzz_target!(|data: &[u8]| {
    let Some(sep) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let (html, rest) = data.split_at(sep);
    let Some((&mode, expr_bytes)) = rest[1..].split_first() else {
        return;
    };
    let Some(expr) = Expr::new(expr_bytes) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };

    unsafe {
        let p = mkr_parse_html(html.as_ptr(), html.len(), false);
        if p.is_null() {
            return;
        }
        run(p, text, mode & 1 != 0);
        mkr_parsed_destroy(p);
    }
});

/// Evaluate over `p` the way the glue's `context_for` does for a Document
/// receiver. Every engine handle is dropped before the caller destroys `p`.
unsafe fn run(p: *mut Parsed, text: VerifiedText, lax: bool) {
    if !mkr_parsed_dom_index_build(p) {
        return;
    }
    // An lxb_html_document_t leads with its DOM document, which leads with its
    // node, so the document is also the context node.
    let doc = mkr_parsed_html_doc(p) as *mut c_void;
    let Some(ctx) = OwnedContext::new(
        doc,
        doc,
        Backend::Html {
            index: mkr_parsed_element_index(p),
        },
    ) else {
        return;
    };
    ctx_set_unprefixed_lax(ctx.as_ptr(), lax as core::ffi::c_int);

    // As tight as `xml_xpath`'s: the fuzzer controls the document here too.
    let l = limits(ctx.as_ptr());
    l.max_eval_ops = 20_000;
    l.max_nodeset_size = 1024;
    l.max_string_bytes = 4096;

    for (prefix, uri) in [
        (&b"svg"[..], &b"http://www.w3.org/2000/svg"[..]),
        (b"math", b"http://www.w3.org/1998/Math/MathML"),
        (b"h", b"http://www.w3.org/1999/xhtml"),
    ] {
        if let (Some(pr), Some(u)) = (
            VerifiedText::from_bytes(prefix),
            VerifiedText::from_bytes(uri),
        ) {
            xpath_register_ns(ctx.as_ptr(), pr, u);
        }
    }

    let Some(ast) = parse(&ctx, text) else {
        return;
    };
    evaluate_both(&ctx, &ast);
}
