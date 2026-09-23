//! `Makiri::XPathContext`'s Ruby methods.
//!
//!   `XPathContext.new(node, namespace_matching: :strict)`, `#evaluate(expr,
//!   handler = nil)`, `#register_namespace(prefix, uri)` / `#register_ns`,
//!   `#register_variable(name, value)`, `#node=`
//!
//! The context itself - its TypedData, its AST cache, and the refusals that
//! protect a running evaluate - is [`crate::bridge::xpath::XPathCtx`]; this
//! module reads the arguments and registers the methods.

#![forbid(unsafe_code)]

use magnus::{function, method, prelude::*, Error, RClass, RHash, Ruby, Value};

use crate::bridge::ruby::{is_kind_of, method_receiver};
use crate::bridge::xpath::XPathCtx;
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
        let a = magnus::scan_args::scan_args::<(Value,), (), (), (), RHash, ()>(args)?;
        let node = a.required.0;
        let kw = Keywords::scan(ruby, a.keywords)?;
        expect_node(ruby, node)?;

        let obj = XPathCtx::create(ruby, node, kw.lax)?;
        if let Some(bindings) = kw.bindings {
            /* A registration that fails leaves `obj` to the GC, never a context
             * with half its prefixes. */
            register_bindings(<&XPathCtx>::try_convert(obj)?, bindings)?;
        }
        Ok(obj)
    })
}

/// `#node=` - rebind the context node, so one context can evaluate relative
/// expressions against several nodes of its document.
fn set_node(ruby: &Ruby, ctx: &XPathCtx, node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        expect_node(ruby, node)?;
        ctx.set_node(ruby, node)?;
        Ok(node)
    })
}

/// `#evaluate(expr, handler = nil)`.
fn evaluate(ruby: &Ruby, ctx: &XPathCtx, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
        let handler = a.optional.0.unwrap_or(ruby.qnil().as_value());
        ctx.evaluate(ruby, a.required.0, handler)
    })
}

/// `#register_namespace(prefix, uri)` -> self.
fn register_namespace(ctx: &XPathCtx, prefix: Value, uri: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        ctx.register_namespace(prefix, uri)?;
        Ok(method_receiver())
    })
}

/// `#register_variable(name, value)` -> self.
fn register_variable(ctx: &XPathCtx, name: Value, value: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        ctx.register_variable(name, value)?;
        Ok(method_receiver())
    })
}

/// From `Init_makiri`, after the classes exist.
pub fn init_xpath_context() {
    let klass =
        RClass::from_value(CLASS_XPATH_CONTEXT.value()).expect("Makiri::XPathContext is a Class");
    klass
        .define_singleton_method("new", function!(s_new, -1))
        .expect("XPathContext.new");
    klass
        .define_method("evaluate", method!(evaluate, -1))
        .expect("#evaluate");
    klass
        .define_method("register_namespace", method!(register_namespace, 2))
        .expect("#register_namespace");
    klass
        .define_method("register_variable", method!(register_variable, 2))
        .expect("#register_variable");
    klass
        .define_method("node=", method!(set_node, 1))
        .expect("#node=");
}
