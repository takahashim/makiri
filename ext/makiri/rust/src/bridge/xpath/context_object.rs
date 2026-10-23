//! `Makiri::XPathContext`: the wrapped object that holds a context and the
//! compiled ASTs it has parsed, and the operations on it. The Ruby methods - argument
//! reading, registration - are `glue::xpath_context`'s.

#![allow(unsafe_code)]

use core::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::bridge::ruby::makiri_error;
use crate::bridge::typed::{Held, Hooks, Tracer, TypedType};
use magnus::{Error, Value};

use crate::bridge::string::ruby_try_verified_text;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::{keepalive_document, node_token_in, NotInDocument};
use crate::falloc::{try_to_boxed_slice, Reserve};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::ContextError;

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
/// Each AST is boxed, so a pointer to one survives the map growing, and
/// nothing is removed before the context itself is freed: an evaluate reads
/// its AST through a pointer (`QueryAst::cached`) after the cache's borrow is
/// released.
struct AstCache(HashMap<Box<[u8]>, Box<Ast>>);

/// `Makiri::XPathContext`.
///
/// The two Ruby values it keeps alive are [`Held`]s, stored only once the
/// object is wrapped ([`XPathCtx::create`]) and traced by [`Hooks::trace`].
/// `document` never changes; `node` does (`#node=`), which is why it - like
/// every `Held` - is a cell rather than living in the `RefCell`: the GC
/// reads both through `&self`, and a GC can land while a mutable borrow is
/// outstanding - the mistake that had to be fixed in `glue::node_set`.
///
/// The engine context sits outside the `RefCell` too: everything done with it
/// takes `&Cx`. No Ruby runs while it evaluates (Makiri runs no
/// custom-function handler), and the context itself refuses a change that
/// would disturb a walk all the same.
pub struct XPathCtx {
    /// Keepalive: owns the DOM arena.
    document: Held,
    /// Keepalive: the context node's wrapper.
    node: Held,
    cache: RefCell<AstCache>,
    /// The bytes the cached ASTs hold, as reported to the GC - kept outside
    /// the `RefCell` because `memsize` reads it from the GC, which can land
    /// while the cache is borrowed.
    cache_bytes: Cell<usize>,
    ctx: Cx,
}

impl Hooks for XPathCtx {
    fn trace(&self, tracer: &Tracer) {
        tracer.pinned(&self.document);
        tracer.pinned(&self.node);
    }

    fn memsize(&self) -> usize {
        core::mem::size_of::<Self>().saturating_add(self.cache_bytes.get())
    }
}

/// `Makiri::XPathContext`'s data type.
pub static XPATH_CTX_TYPE: TypedType<XPathCtx> = TypedType::base(c"Makiri::XPathContext".as_ptr());

/// An `XPathContext` method's receiver: the object, checked to be one, read
/// as its [`XPathCtx`] for the call.
pub struct XPathCtxSelf(Value);

impl magnus::TryConvert for XPathCtxSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        XPATH_CTX_TYPE.get(&value)?;
        Ok(XPathCtxSelf(value))
    }
}

impl core::ops::Deref for XPathCtxSelf {
    type Target = XPathCtx;
    fn deref(&self) -> &XPathCtx {
        XPATH_CTX_TYPE.get_known(&self.0)
    }
}

impl XPathCtxSelf {
    /// Rebind the context node to `rb_node`, a Makiri node of the same
    /// document. Namespace and variable registrations are preserved.
    pub fn set_node(&self, rb_node: Value) -> Result<(), Error> {
        const BUSY: &str = "cannot change the context node while the context is evaluating";
        let token = node_token_in(rb_node, self.document()).map_err(|e| match e {
            NotInDocument::Unusable(e) => e,
            NotInDocument::Foreign => makiri_error("context node must belong to the same document"),
        })?;
        self.ctx
            .set_context_node(token)
            .map_err(|e| refused(e, BUSY, BUSY))?;
        /* Kept alive once it is the context node: a refusal above leaves the
         * old node and its keepalive as they were. */
        XPATH_CTX_TYPE.with_store(self.0, |c, store| c.node.replace(store.hold(rb_node)));
        Ok(())
    }
}

impl Drop for XPathCtx {
    /// Take the cache's report back, or the GC keeps counting freed ASTs as
    /// live - the same balance `DocData` keeps for its arena.
    fn drop(&mut self) {
        crate::bridge::ruby::report_external_bytes(-(self.cache_bytes.get() as isize));
    }
}

impl XPathCtx {
    /// Count `added` more cached bytes, to `size` and to the GC: a context
    /// holding up to `AST_CACHE_MAX` parsed expressions is more than the few
    /// dozen bytes of its struct, and without the report the GC never sees it.
    fn account_cache(&self, added: usize) {
        if added != 0 {
            self.cache_bytes
                .set(self.cache_bytes.get().saturating_add(added));
            crate::bridge::ruby::report_external_bytes(added as isize);
        }
    }

    fn cache(&self) -> Result<core::cell::RefMut<'_, AstCache>, Error> {
        self.cache
            .try_borrow_mut()
            .map_err(|_| makiri_error("XPath context is already in use"))
    }
}

const NS_BUSY: &str = "cannot register a namespace while the context is evaluating";
const NS_FAILED: &str = "failed to register namespace";

/// A context's refusal as the exception it raises: `busy` when an evaluate is
/// running on it, `failed` when an allocation failed, and a cap message
/// otherwise.
fn refused(error: ContextError, busy: &'static str, failed: &'static str) -> Error {
    let msg = match error {
        ContextError::Evaluating => busy,
        ContextError::Oom => failed,
        ContextError::TooMany => "too many registrations on this XPath context",
    };
    makiri_error(msg)
}

/* ------------------------------------------------------------------ */
/* the context's operations                                           */
/* ------------------------------------------------------------------ */
/* What `glue::xpath_context`'s Ruby methods are built on. */

impl XPathCtx {
    /// A new context rooted at `rb_node`, a node of its keepalive Document.
    pub fn create(rb_node: Value, lax: bool) -> Result<Value, Error> {
        let document = keepalive_document(rb_node)?;
        let mut ctx = context_for(rb_node, document)?;
        ctx.set_lax(lax);
        // SAFETY: under the GVL. The struct is built holding no value - the
        // two are stored once it is wrapped - and owning no allocation, as
        // `wrap` asks: a fresh `Cx`'s registrations and the cache are empty
        // collections, which allocate nothing until the first insert, so a
        // `NoMemoryError` from the wrap leaks the struct's own memory alone.
        let obj = unsafe {
            XPATH_CTX_TYPE.wrap(
                CLASS_XPATH_CONTEXT.raw(),
                || XPathCtx {
                    document: Held::empty(),
                    node: Held::empty(),
                    cache: RefCell::new(AstCache(HashMap::new())),
                    cache_bytes: Cell::new(0),
                    ctx,
                },
                |c, store| {
                    c.document = store.hold(document);
                    c.node = store.hold(rb_node);
                },
            )
        };
        // SAFETY: the object just wrapped, live on this frame.
        Ok(unsafe { crate::bridge::ruby::value(obj?) })
    }

    /// The keepalive Document.
    fn document(&self) -> Value {
        // SAFETY: a field `trace` traces, of the receiver the caller holds.
        unsafe { self.document.get() }
    }

    /// Evaluate `expr`, parsed once and cached.
    pub fn evaluate(&self, expr: Value) -> Result<Value, Error> {
        let document = self.document();

        /* The cache borrow is taken for the lookup only, and released before the
         * evaluation. */
        let ast = {
            let ev = ruby_verified_text(expr, "XPath expression")?;
            let mut cache = self.cache()?;
            let parsed = cached_ast(&mut cache, self.ctx.limits(), ev);
            /* Release the borrow before building the exception. */
            drop(cache);
            let (ast, added) = parsed.map_err(|error| xpath_error(&error))?;
            self.account_cache(added);
            ast
        };
        evaluate_to_ruby(&self.ctx, ast, document, Answer::All)
    }

    /// The engine caps this context evaluates under, for a caller that must
    /// check a string against them before binding it.
    pub fn limits(&self) -> crate::xpath::limits::Limits {
        self.ctx.limits()
    }

    /// Bind `prefix` to `uri` for every later evaluate, for bytes a caller has
    /// already put through the text contract (`glue::query::bind_pair`).
    pub fn bind_namespace(&self, prefix: &[u8], uri: &[u8]) -> Result<(), Error> {
        self.ctx
            .register_ns(prefix, uri) /* copies both */
            .map_err(|e| refused(e, NS_BUSY, NS_FAILED))
    }

    /// Bind `$name` to the String `value` for every later evaluate.
    pub fn register_variable(&self, name: Value, value: Value) -> Result<(), Error> {
        const BUSY: &str = "cannot register a variable while the context is evaluating";
        const FAILED: &str = "failed to register variable";
        /* The value gets the stricter engine-string check, which adds the
         * byte cap on top of the no-NUL / valid-UTF-8 contract. */
        let sv = crate::bridge::ruby::string_arg(value)?;
        let nv = ruby_verified_text(name, "variable name")?;
        let vv = ruby_try_verified_text(sv, self.ctx.limits().max_string_bytes)
            .map_err(|reason| makiri_error(format!("invalid variable value: {reason}")))?;
        self.ctx
            .register_variable(nv.as_bytes(), vv.as_bytes()) /* copies both */
            .map_err(|e| refused(e, BUSY, FAILED))
    }
}

/// The AST for `expr`, from the cache or freshly parsed and cached, with the
/// bytes it newly added to the cache (0 for a hit, or an AST not cached - one
/// the query then owns).
///
/// A cached AST is handed out borrowed: the cache frees nothing before the
/// context goes ([`AstCache`]), so it outlives the evaluation it is for, which
/// runs on this live context.
#[allow(clippy::result_large_err)]
fn cached_ast(
    cache: &mut AstCache,
    limits: crate::xpath::limits::Limits,
    expr: RubyText,
) -> Result<(QueryAst, usize), crate::engine_error::Error> {
    /* `expr` owns a copy of the String's bytes, so the key cannot change. */
    let key = expr.as_bytes();
    if let Some(ast) = cache.0.get(key) {
        // SAFETY: cached, so alive and unchanged while the context is.
        return Ok((unsafe { QueryAst::cached(ast) }, 0));
    }

    let ast = parse_with_limits(limits, expr.text())?;
    if cache.0.len() >= AST_CACHE_MAX || cache.0.falloc_reserve(1).is_err() {
        return Ok((QueryAst::owned(ast), 0));
    }
    let Some(owned_key) = try_to_boxed_slice(key) else {
        return Ok((QueryAst::owned(ast), 0));
    };
    let added = owned_key.len().saturating_add(ast.heap_estimate());
    /* The slot was reserved above, so the vacant entry's insert allocates
     * nothing. The AST is borrowed from the map AFTER the insert, not through
     * a pointer taken before it: moving the `Box` into the map asserts unique
     * ownership of the AST, which a pointer derived earlier does not survive
     * (the same Stacked Borrows hazard `wrapper::Content` avoids). */
    let ast = cache.0.entry(owned_key).or_insert(ast);
    // SAFETY: just cached, so alive and unchanged while the context is.
    Ok((unsafe { QueryAst::cached(ast) }, added))
}
