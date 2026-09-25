//! CSS over XML: the private `_css` / `_at_css` / `_css_matches` primitives the
//! Ruby `#css` / `#at_css` / `#matches?` call once they have collected the
//! document's namespaces (`lib/makiri/xml/node_methods.rb`).
//!
//! A selector is compiled to the XPath engine's AST and run through the SAME
//! evaluator as `#xpath`, so case-sensitivity, namespaces, budgets and document
//! order are identical. The namespaces arrive as a normalised `{prefix => uri}`
//! Hash; a default namespace comes under the synthetic `"xmlns"` prefix, which a
//! bare type selector binds to.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RHash, Ruby, Value};

use crate::bridge::string::ruby_verified_text;
use crate::bridge::wrapper::keepalive_document;
use crate::bridge::xpath::{evaluate_query, xpath_error, Answer, Cx};
use crate::css::{CssNs, Form, DEFAULT_NS_PREFIX};
use crate::glue::query::{query_context, run_query, QueryArgs};
use crate::init::MOD_XML_NODE_METHODS;
use crate::xpath::ast::Ast;
use crate::xpath::ctx::XPathValue;
use crate::xpath::msg::Status;

/// The query arguments for a selector and its namespace Hash.
fn css_args(ruby: &Ruby, selector: Value, ns: Value) -> QueryArgs {
    QueryArgs {
        text: selector,
        namespaces: RHash::from_value(ns),
        handler: ruby.qnil().as_value(),
        lax: false,
    }
}

/// Whether the namespace Hash binds the default namespace.
fn default_namespace(ruby: &Ruby, q: &QueryArgs) -> bool {
    q.namespaces
        .is_some_and(|h| matches!(h.get(ruby.str_new(DEFAULT_NS_PREFIX)), Some(v) if !v.is_nil()))
}

/// Compile the selector under `ctx`, whose namespaces are already registered.
fn compile(ruby: &Ruby, ctx: &Cx, q: &QueryArgs, form: Form) -> Result<Box<Ast>, Error> {
    let cns = CssNs {
        default_namespace: default_namespace(ruby, q),
    };
    let sv = ruby_verified_text(q.text, c"CSS selector")?;
    let mut budget = crate::xpath::limits::Budget::with_limits(ctx.limits());
    /* `sv` holds the selector String rooted; the compile allocates through
     * falloc only - no Ruby runs in it. */
    let gvl = crate::bridge::gvl::held(ruby);
    let ast = crate::css::compile_owned(&gvl, sv.as_verified(), &cns, form, &mut budget);
    drop(sv);
    ast.map_err(|_| compile_error(q.text, &budget.take_error()))
}

/// A failed compile as its exception: `Makiri::CSS::SyntaxError` for a selector
/// that does not parse or lower - with the lowering's reason, when it gave one -
/// and the XPath mapping otherwise.
fn compile_error(selector: Value, error: &crate::xpath::msg::Error) -> Error {
    if error.status != Status::Syntax {
        return xpath_error(error);
    }
    let reason = error.message().map(|m| m.to_string_lossy().into_owned());
    crate::glue::css::syntax_error(selector, reason.as_deref())
}

fn css_run(ruby: &Ruby, rb_self: Value, q: QueryArgs, answer: Answer) -> Result<Value, Error> {
    let document = keepalive_document(rb_self)?;
    let ctx = query_context(rb_self, document, &q)?;
    let ast = compile(ruby, &ctx, &q, Form::Select)?;
    run_query(ctx, ast, q.handler, document, answer)
}

fn css(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| css_run(ruby, rb_self, css_args(ruby, selector, ns), Answer::All))
}

fn at_css(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        css_run(ruby, rb_self, css_args(ruby, selector, ns), Answer::First)
    })
}

/// `#matches?(selector)`: does THIS node match?
///
/// Compiled as a self-test (`Form::SelfTest`) and evaluated from the node, so it
/// walks the node's ancestors and siblings rather than selecting across the
/// whole document - the same question Lexbor's `match_node` answers for HTML.
fn css_matches(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let q = css_args(ruby, selector, ns);
        let document = keepalive_document(rb_self)?;
        let ctx = query_context(rb_self, document, &q)?;
        let ast = compile(ruby, &ctx, &q, Form::SelfTest)?;
        let value = evaluate_query(&ctx, &ast, q.handler, document, Answer::All)?;
        Ok(matches!(value, XPathValue::Boolean(true)))
    })
}

/// The private primitives, on the XML node-method module. From `Init_makiri`.
pub fn init_xml_css() -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.module();
    m.define_private_method("_css", method!(css, 2))?;
    m.define_private_method("_at_css", method!(at_css, 2))?;
    m.define_private_method("_css_matches", method!(css_matches, 2))?;
    Ok(())
}
