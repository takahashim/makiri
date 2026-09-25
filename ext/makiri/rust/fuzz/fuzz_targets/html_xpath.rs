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

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::lexbor::adapter::post_parse::{parse_html, HtmlParsed};

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
        let Some(mut p) = parse_html(html, false) else {
            return;
        };
        run(&mut p, text, mode & 1 != 0);
    }
});

/// Evaluate over `p` the way the glue's `context_for` does for a Document
/// receiver. Every engine handle is dropped before the caller destroys `p`.
unsafe fn run(p: &mut HtmlParsed, text: VerifiedText, lax: bool) {
    // An lxb_html_document_t leads with its DOM document, which leads with its
    // node, so the document is also the context node.
    let doc = p.raw_doc().as_ptr();
    if p.ensure_dom_index().is_err() {
        return;
    }
    // SAFETY: the caller destroys `p` only after the context is dropped, and
    // nothing changes the document in between; `doc` is that live document's
    // node.
    let Ok(mut ctx) = makiri::lexbor::xpath::context(p, Token::html(doc)) else {
        return;
    };
    ctx.set_lax(lax);

    // As tight as `xml_xpath`'s: the fuzzer controls the document here too.
    let l = ctx.limits_mut();
    l.max_eval_ops = 20_000;
    l.max_nodeset_size = 1024;
    l.max_string_bytes = 4096;
    /* A cache smaller than one value, so the uncached comparison path runs. */
    l.max_cache_bytes = 256;

    for (prefix, uri) in [
        (&b"svg"[..], &b"http://www.w3.org/2000/svg"[..]),
        (b"math", b"http://www.w3.org/1998/Math/MathML"),
        (b"h", b"http://www.w3.org/1999/xhtml"),
    ] {
        let _ = ctx.register_ns(prefix, uri);
    }

    let Some(ast) = parse(&ctx, text) else {
        return;
    };
    evaluate_both(&ctx, &ast);
}
