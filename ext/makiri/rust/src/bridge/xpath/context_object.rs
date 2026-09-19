//! `Makiri::XPathContext`: the TypedData that holds a context, the compiled
//! ASTs it has parsed, and its Ruby methods.

#![allow(unsafe_code)]

use core::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use magnus::gc::Marker;

use crate::bridge::ruby::makiri_error;
use magnus::rb_sys::AsRawValue;
use magnus::value::{Opaque, ReprValue};
use magnus::{method, DataTypeFunctions, Error, RClass, Ruby, TypedData, Value};

use crate::bridge::ruby::VALUE;
use crate::bridge::string::ruby_try_verified_text;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::{keepalive_document, node_raw};
use crate::falloc::{try_to_boxed_slice, MapInsert, Reserve};
use crate::init::CLASS_NODE;
pub use crate::init::CLASS_XPATH_CONTEXT;
use crate::xpath::ast::Ast;
use crate::xpath::ctx::ContextError;
use crate::xpath::limits::Budget;
use crate::xpath::msg::XP_ERR_OOM;

use super::*;

/* ================================================================== *
 * the XPathContext wrapper and the Ruby custom-function bridge        *
 * ================================================================== */

/// An `XPathContext` is typically reused to run the same handful of expressions
/// many times, so each is parsed once and the AST re-evaluated (the evaluator
/// resets the per-eval counters and keeps its per-evaluate state off the AST,
/// so a cached AST is safely reusable). Bounded, so a context fed unbounded
/// distinct expressions cannot grow without limit.
const AST_CACHE_MAX: usize = 1024;

/* ------------------------------------------------------------------ */
/* the context wrapper                                                */
/* ------------------------------------------------------------------ */

/// The compiled ASTs this context has parsed, keyed by the expression bytes.
///
/// The C scanned a flat array with `strcmp`; a map answers the same question
/// without the scan. The bound and the fallback are unchanged: past
/// [`AST_CACHE_MAX`] nothing more is cached and the caller frees the AST it was
/// handed.
///
/// Each AST is boxed, so a pointer to one survives the map growing: a handler
/// can evaluate a new expression on this context mid-walk, inserting into the
/// map while the outer evaluate still reads its AST. Nothing is ever removed
/// before the context itself is freed.
struct AstCache(HashMap<Box<[u8]>, Box<Ast>>);

/// `Makiri::XPathContext`.
///
/// `document` never changes and so is a plain field. `node` DOES change (via
/// `#node=`) and is also marked, which is why it is a `Cell` rather than living
/// in the `RefCell`: `mark` must reach both on every GC, and a GC can land while
/// a mutable borrow is outstanding - the mistake that had to be fixed in
/// `glue::node_set`.
///
/// The engine context sits outside the `RefCell` too: everything done with it
/// takes `&Context`, so a handler re-entering mid-walk can evaluate again on it,
/// and the context itself refuses the changes that would disturb the walk.
#[derive(TypedData)]
#[magnus(class = "Makiri::XPathContext", mark, size, free_immediately)]
struct XPathCtx {
    /// Keepalive: owns the DOM arena.
    document: Opaque<Value>,
    /// Keepalive: the context node's wrapper.
    node: Cell<Opaque<Value>>,
    cache: RefCell<AstCache>,
    ctx: Cx,
}

/* SAFETY: every access holds the GVL, which serialises Ruby threads. magnus's
 * TypedData needs the bound because a wrapped value may be freed on whichever
 * thread runs the GC - still under the GVL. */
unsafe impl Send for XPathCtx {}

impl DataTypeFunctions for XPathCtx {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.document);
        marker.mark(self.node.get());
    }

    fn size(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

impl XPathCtx {
    fn cache(&self) -> Result<core::cell::RefMut<'_, AstCache>, Error> {
        self.cache
            .try_borrow_mut()
            .map_err(|_| makiri_error("XPath context is already in use"))
    }
}

/// A context's refusal as the exception it raises: `busy` when an evaluate is
/// running on it, `failed` otherwise.
fn refused(error: ContextError, busy: &'static str, failed: &'static str) -> Error {
    let msg = match error {
        ContextError::Evaluating => busy,
        ContextError::Failed => failed,
    };
    makiri_error(msg)
}

/* ------------------------------------------------------------------ */
/* context construction                                               */
/* ------------------------------------------------------------------ */

/// The three symbols the keyword check compares against, interned once.
/// `to_symbol` is a lookup, and this runs on the per-call path.
fn kw_symbols() -> (VALUE, VALUE, VALUE) {
    static SYMS: OnceLock<(VALUE, VALUE, VALUE)> = OnceLock::new();
    *SYMS.get_or_init(|| {
        // Every caller is a Ruby method entered with the GVL. Symbols are
        // interned once and are immortal for the Ruby VM's lifetime.
        let sym = |s: &str| crate::bridge::ruby::symbol(s).as_raw();
        (sym("namespace_matching"), sym("strict"), sym("lax"))
    })
}

/// Resolve the `namespace_matching:` keyword to the unprefixed-lax flag.
///
/// `:strict` (the default) resolves an unprefixed name test in the HTML
/// namespace, which is what browsers do; `:lax` makes it namespace-agnostic.
pub fn ns_matching_lax(ruby: &Ruby, opts: magnus::RHash) -> Result<bool, Error> {
    if opts.is_empty() {
        return Ok(false);
    }
    let (key, strict, lax) = kw_symbols();
    let Some(v) = opts.get(unsafe { crate::bridge::ruby::value(key) }) else {
        return Ok(false);
    };
    if v.is_nil() || v.as_raw() == strict {
        return Ok(false);
    }
    if v.as_raw() == lax {
        return Ok(true);
    }
    Err(Error::new(
        ruby.exception_arg_error(),
        format!(
            "namespace_matching: must be :strict or :lax, got {}",
            v.inspect()
        ),
    ))
}

/// `XPathContext.new(node, namespace_matching: :strict)`.
fn ctx_s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let rb_node = a.required.0;
    let lax = ns_matching_lax(ruby, a.keywords)?;

    if !is_kind_of(rb_node, &CLASS_NODE) {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::Node",
        ));
    }
    let document = keepalive_document(rb_node)?;
    let mut ctx = context_for(rb_node, document)?;
    ctx.set_lax(lax);

    let obj = ruby
        .wrap(XPathCtx {
            document: document.into(),
            node: Cell::new(rb_node.into()),
            cache: RefCell::new(AstCache(HashMap::new())),
            ctx,
        })
        .as_value();
    /* While `wrap` allocates the object - a GC point - both values are held only
     * by the boxed struct, where no mark sees them. Using them afterwards keeps
     * them on the machine stack across that call, pinned by the conservative
     * scan, so compaction cannot move them out from under the stored copies. */
    core::hint::black_box((document, rb_node));
    Ok(obj)
}

/// `#node=` - rebind the context node, so one context can evaluate relative
/// expressions against several nodes. Namespace and variable registrations are
/// preserved. The node must be in the same document.
fn ctx_set_node(ruby: &Ruby, rb_self: &XPathCtx, rb_node: Value) -> Result<Value, Error> {
    if !is_kind_of(rb_node, &CLASS_NODE) {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::Node",
        ));
    }
    const BUSY: &str =
        "cannot change the context node while evaluating (re-entrant mutation from a handler)";
    /* The engine refuses this too; asked first so a handler's re-entrant call
     * reads as busy rather than as whatever the checks below would say. */
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, BUSY));
    }
    if keepalive_document(rb_node)?.as_raw() != ruby.get_inner(rb_self.document).as_raw() {
        return Err(makiri_error(
            "context node must belong to the same document",
        ));
    }
    rb_self.node.set(rb_node.into()); /* keepalive; marked above */
    /* Same-document is verified, so rb_node is a node of the context's
     * document; mint the token for whichever backend that document is. */
    let raw = node_raw(rb_node)?;
    // SAFETY: a live node of this context's document.
    let token = unsafe { node_token(rb_self.ctx.token_kind(), raw) }
        .ok_or_else(|| makiri_error("the context has no document"))?;
    rb_self
        .ctx
        .set_context_node(token)
        .map_err(|e| refused(e, BUSY, BUSY))?;
    Ok(rb_node)
}

/* ------------------------------------------------------------------ */
/* evaluate                                                           */
/* ------------------------------------------------------------------ */

/// The compiled AST for `expr`, parsing and caching it on first use.
///
/// Returns a pointer to the AST plus its owner when it could not be cached. A
/// cached AST lives as long as the context (see [`AstCache`]).
#[allow(clippy::result_large_err)]
fn cached_ast(
    cache: &mut AstCache,
    limits: crate::xpath::limits::Limits,
    expr: RubyText,
) -> Result<(*const Ast, Option<Box<Ast>>), crate::xpath::msg::Error> {
    // SAFETY: `expr` holds its String rooted for this lookup.
    let key = unsafe { expr.bytes() };
    if let Some(ast) = cache.0.get(key) {
        return Ok((&**ast as *const Ast, None));
    }

    /* Each parse charges a budget of its own, made from the context's caps. */
    let mut budget = Budget::with_limits(limits);
    let Ok(ast) = crate::xpath::parse::parse_owned(expr.as_verified(), &mut budget) else {
        return Err(budget.take_error());
    };
    if cache.0.len() >= AST_CACHE_MAX || cache.0.mkr_reserve(1).is_err() {
        return Ok((&*ast as *const Ast, Some(ast)));
    }
    let Some(owned_key) = try_to_boxed_slice(key) else {
        return Ok((&*ast as *const Ast, Some(ast)));
    };
    if cache.0.mkr_insert(owned_key, ast).is_err() {
        return Err(XPathError::with(
            XP_ERR_OOM,
            format_args!("out of memory caching XPath expression"),
        ));
    }
    let ast = cache.0.get(key).expect("inserted AST");
    Ok((&**ast as *const Ast, None))
}

fn ctx_evaluate(ruby: &Ruby, rb_self: &XPathCtx, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
        let expr = a.required.0;
        let handler = a.optional.0.unwrap_or(ruby.qnil().as_value());
        let document = ruby.get_inner(rb_self.document);

        /* The cache borrow is taken for the lookup ONLY, and released before the
         * evaluation. A handler called mid-walk re-enters this object - to register
         * a namespace, to rebind the node, or to evaluate again on the same context
         * - and re-entrancy is governed by the context itself, which reports the
         * specific refusal (and permits a nested evaluate). Holding the borrow
         * across the walk would turn all four into one generic "already in use",
         * which is how the handler specs first caught this. */
        let (ast, owned) = {
            /* Verify BEFORE borrowing: coercing the expression can run Ruby (`to_s`),
             * which may re-enter this context, and a borrow held across that would
             * turn the re-entry into "already in use". */
            let ev = ruby_verified_text(expr, c"XPath expression")?;
            let mut cache = rb_self.cache()?;
            let parsed = cached_ast(&mut cache, rb_self.ctx.limits(), ev);
            /* Release the borrow before building the exception: that allocates, and
             * a NoMemoryError there would longjmp past the RefMut. */
            drop(cache);
            match parsed {
                Ok((ast, owned)) => (ast, owned),
                Err(error) => return Err(xpath_error(&error)),
            }
        };

        /* A cached AST outlives this call: the context is live (it is `rb_self`),
         * and its cache frees nothing before the context goes. */
        // SAFETY: as above - the AST the cache just handed back.
        let value = evaluate_query(
            &rb_self.ctx,
            unsafe { &*ast },
            handler,
            document,
            Answer::All,
        );
        drop(owned);
        query_result(value?, document, Answer::All)
    })
}

fn ctx_register_ns(rb_self: &XPathCtx, prefix: Value, uri: Value) -> Result<Value, Error> {
    const BUSY: &str =
        "cannot register a namespace while evaluating (re-entrant mutation from a handler)";
    const FAILED: &str = "failed to register namespace";
    /* The engine refuses this too, but only once the arguments are converted -
     * and converting runs `to_s`, arbitrary Ruby, from inside a handler. Asking
     * first answers "busy" before any of that runs. */
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, FAILED));
    }
    let pv = ruby_verified_text(prefix, c"namespace prefix")?;
    let uv = ruby_verified_text(uri, c"namespace URI")?;
    rb_self
        .ctx
        .register_ns(pv.as_verified().as_bytes(), uv.as_verified().as_bytes()) /* copies both */
        .map_err(|e| refused(e, BUSY, FAILED))?;
    Ok(crate::bridge::ruby::method_receiver())
}

fn ctx_register_variable(rb_self: &XPathCtx, name: Value, value: Value) -> Result<Value, Error> {
    const BUSY: &str =
        "cannot register a variable while evaluating (re-entrant mutation from a handler)";
    const FAILED: &str = "failed to register variable";
    /* Before any conversion runs Ruby - see `ctx_register_ns`. */
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, FAILED));
    }
    /* Coerce the value FIRST - to_s allocates, which is a GC point - so no
     * borrowed name bytes are held across it. The value then gets the stricter
     * engine-string check, which adds the byte cap on top of the no-NUL /
     * valid-UTF-8 contract. */
    let sv: Value = value.funcall("to_s", ())?;
    let nv = ruby_verified_text(name, c"variable name")?;
    // SAFETY: `sv` is a live String, and the borrow ends with the check.
    let vv =
        match unsafe { ruby_try_verified_text(sv.as_raw(), rb_self.ctx.limits().max_string_bytes) }
        {
            Ok(vv) => vv,
            Err(reason) => {
                return Err(makiri_error(format!(
                    "invalid variable value: {}",
                    reason.to_string_lossy()
                )));
            }
        };
    rb_self
        .ctx
        .register_variable(nv.as_verified().as_bytes(), vv.as_verified().as_bytes()) /* copies both */
        .map_err(|e| refused(e, BUSY, FAILED))?;
    Ok(crate::bridge::ruby::method_receiver())
}

/// Register `Makiri::XPathContext`. `Node#xpath` is `glue::query`'s.
pub fn init_xpath_context() {
    let klass =
        RClass::from_value(CLASS_XPATH_CONTEXT.value()).expect("Makiri::XPathContext is a Class");
    klass
        .define_singleton_method("new", magnus::function!(ctx_s_new, -1))
        .expect("XPathContext.new");
    klass
        .define_method("evaluate", method!(ctx_evaluate, -1))
        .expect("#evaluate");
    klass
        .define_method("register_namespace", method!(ctx_register_ns, 2))
        .expect("#register_namespace");
    klass
        .define_method("register_variable", method!(ctx_register_variable, 2))
        .expect("#register_variable");
    klass
        .define_method("node=", method!(ctx_set_node, 1))
        .expect("#node=");
}
