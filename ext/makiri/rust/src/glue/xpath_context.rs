//! `Makiri::XPathContext`'s Ruby methods.
//!
//!   `XPathContext.new(node, namespace_matching: :strict)`, `#evaluate(expr)`,
//!   `#register_namespace(prefix, uri)` / `#register_ns`,
//!   `#register_variable(name, value)`, `#node=`
//!
//! The context itself - its wrapped data, its AST cache, and the refusals that
//! protect a running evaluate - is [`crate::bridge::xpath::XPathCtx`]; this
//! module reads the arguments and registers the methods.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::{function, method};

use crate::bridge::ruby::is_kind_of;
use crate::bridge::xpath::{XPathCtx, XPathCtxSelf};
use crate::glue::kwargs::Kwargs;
use crate::glue::query::{register_bindings, Keywords};
use crate::init::{CLASS_NODE, CLASS_XPATH_CONTEXT};

/// `Err(TypeError)` unless `v` is a Makiri node.
fn expect_node(ruby: &Ruby, v: Value) -> Result<(), Error> {
    if is_kind_of(v, &CLASS_NODE) {
        return Ok(());
    }
    Err(Error::new(
        ruby.exception_type_error(),
        "expected a Makiri::Node",
    ))
}

/// `XPathContext.new(node, namespace_matching: :strict, **prefix_bindings)`.
///
/// Keywords read exactly as `#xpath`'s do: the mode, and prefix bindings - here
/// registered on the context, so they hold for every later evaluate.
fn s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::with_keywords(1, 0))?;
        let node = a.required(0);
        let kw = Keywords::scan(ruby, Kwargs::from_scan(a.keywords()))?;
        expect_node(ruby, node)?;

        let obj = XPathCtx::create(node, kw.lax)?;
        if let Some(bindings) = kw.bindings {
            /* A registration that fails leaves `obj` to the GC, never a context
             * with half its prefixes. */
            let ctx: XPathCtxSelf = XPathCtxSelf::try_convert(obj)?;
            register_bindings(&ctx, bindings)?;
        }
        Ok(obj)
    })
}

/// `#node=` - rebind the context node, so one context can evaluate relative
/// expressions against several nodes of its document.
fn set_node(ruby: &Ruby, ctx: XPathCtxSelf, node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        expect_node(ruby, node)?;
        ctx.set_node(node)?;
        Ok(node)
    })
}

/// `#evaluate(expr)`.
fn evaluate(ctx: XPathCtxSelf, expr: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| ctx.evaluate(expr))
}

/// `#register_namespace(prefix, uri)` -> self.
fn register_namespace(ctx: XPathCtxSelf, prefix: Value, uri: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let cap = ctx.limits().max_string_bytes;
        crate::glue::query::bind_pair(prefix, uri, cap, |p, u| ctx.bind_namespace(p, u))?;
        Ok(ctx.value())
    })
}

/// `#register_variable(name, value)` -> self.
fn register_variable(ctx: XPathCtxSelf, name: Value, value: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        ctx.register_variable(name, value)?;
        Ok(ctx.value())
    })
}

/// From `Init_makiri`, after the classes exist.
pub fn init_xpath_context() -> Result<(), Error> {
    let klass = CLASS_XPATH_CONTEXT.defined()?;
    klass.define_singleton_method("new", function!(s_new, -1))?;
    klass.define_method("evaluate", method!(evaluate, 1))?;
    klass.define_method("register_namespace", method!(register_namespace, 2))?;
    klass.define_method("register_variable", method!(register_variable, 2))?;
    klass.define_method("node=", method!(set_node, 1))?;
    Ok(())
}
