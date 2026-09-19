//! `Node#xpath` / `#at_xpath`, and the one path every query takes from a
//! compiled expression to a Ruby value.
//!
//!   `Node#xpath(expr, handler = nil, namespace_matching:)` / `#at_xpath(...)`
//!
//! `Makiri::XPathContext` - a TypedData holding a context and an AST cache - and
//! the custom-function handler bridge (`rb_protect`, raw `VALUE`s, `extern "C"`)
//! touch the raw Ruby ABI, so they live in [`crate::bridge::xpath`]; this module
//! is the Ruby surface built on them, and holds no unsafe.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};

use crate::bridge::wrapper::keepalive_document;
use crate::bridge::xpath::{
    context_for, evaluate_query, ns_matching_lax, parse_query, query_result, Cx,
};
use crate::init::MOD_HTML_NODE_METHODS;
use crate::xpath::ast::Ast;

/// Evaluate `ast` under `ctx` and convert the value for Ruby.
///
/// Both are taken by value and dropped BEFORE the conversion, which allocates
/// Ruby objects and so may raise or collect: the value owns its data and
/// references neither, so nothing is held that a raise would leak. The callers
/// used to spell that order out each time, in a comment.
pub fn run_query(
    ctx: Cx,
    ast: Box<Ast>,
    handler: Value,
    document: Value,
    first_only: bool,
) -> Result<Value, Error> {
    let value = evaluate_query(&ctx, &ast, handler, document, first_only);
    drop(ast);
    drop(ctx);
    query_result(value?, document, first_only)
}

/// A throwaway context per call, so `Node#xpath` caches nothing;
/// `Makiri::XPathContext` is what a caller reaches for when many queries share
/// one namespace set and one set of compiled expressions.
fn node_xpath_run(
    rb_self: Value,
    expr: Value,
    handler: Value,
    lax: bool,
    first_only: bool,
) -> Result<Value, Error> {
    let document = keepalive_document(rb_self)?;
    let mut ctx = context_for(rb_self, document)?;
    ctx.set_lax(lax);
    let ast = parse_query(&ctx, expr)?;
    run_query(ctx, ast, handler, document, first_only)
}

/// `(expression, handler, lax)` from the argument list.
///
/// The one-argument call - `node.xpath(expr)`, much the commonest - is answered
/// before `scan_args` runs at all. That is not a micro-optimisation: `scan_args`
/// with a keyword type allocates an empty Hash even when no keywords were
/// passed, and `at_xpath` spends about 650ns per call in total, so the
/// allocation and the symbol lookups behind it measured ~32% of it.
fn scan_query_args(ruby: &Ruby, args: &[Value]) -> Result<(Value, Value, bool), Error> {
    if let [expr] = args {
        return Ok((*expr, ruby.qnil().as_value(), false));
    }
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), magnus::RHash, ()>(
        args,
    )?;
    let lax = ns_matching_lax(ruby, a.keywords)?;
    Ok((
        a.required.0,
        a.optional.0.unwrap_or(ruby.qnil().as_value()),
        lax,
    ))
}

fn node_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let (expr, handler, lax) = scan_query_args(ruby, args)?;
        node_xpath_run(rb_self, expr, handler, lax, false)
    })
}

/// The first matching node for a node-set result, or the scalar otherwise.
fn node_at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let (expr, handler, lax) = scan_query_args(ruby, args)?;
        node_xpath_run(rb_self, expr, handler, lax, true)
    })
}

/// Register `Makiri::XPathContext` and `Node#xpath` / `#at_xpath`. From
/// `Init_makiri`.
pub fn init_xpath() {
    crate::bridge::xpath::init_xpath_context();
    let m = magnus::RModule::from_value(MOD_HTML_NODE_METHODS.value())
        .expect("Makiri::HTML::NodeMethods");
    m.define_method("xpath", method!(node_xpath, -1))
        .expect("#xpath");
    m.define_method("at_xpath", method!(node_at_xpath, -1))
        .expect("#at_xpath");
}
