//! The Ruby boundary of the native XPath engine (glue/ruby_xpath.c).
//!
//!   `Makiri::XPathContext.new(node, namespace_matching: :strict)`
//!   `ctx#evaluate(expr, handler = nil)` -> NodeSet | String | Float | bool
//!   `ctx#register_namespace` / `#register_variable` / `#node=`
//!   `Node#xpath(expr, handler = nil)` / `Node#at_xpath(...)`
//!
//! The engine owns its result buffers; a result is copied into Ruby objects and
//! the native value cleared immediately.
//!
//! # Evaluation deliberately holds the GVL
//!
//! The engine and the DOM are not thread-safe against concurrent mutation, and
//! holding the GVL across every evaluation makes that safe *by construction*: it
//! serialises all Ruby-thread C code, so an XPath walk can never run in parallel
//! with a tree mutation, with another evaluation on the same context, or with a
//! `register_*` / `node=` on it. No locking is needed, and none is used.
//! (Parsing a document still releases the GVL - a freshly parsed document is not
//! yet shared, so it has no such hazard.) Releasing it for the handler-free case
//! was measured to scale across threads, but the locking that would make a
//! GVL-released walk safe against shared-document mutation was judged not worth
//! the verification burden.
//!
//! # Shared with the XML query glue
//!
//! [`xpath_error`] and [`value_to_ruby`] are used by the XML query
//! glue as well, so an engine failure or value maps to the same Ruby object
//! whichever entry point produced it.

#![allow(clippy::missing_safety_doc)]

use crate::falloc::{try_to_boxed_slice, MapInsert, Reserve};
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_void};
use std::collections::HashMap;
use std::sync::OnceLock;

use magnus::gc::Marker;
use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::value::{Opaque, ReprValue};
use magnus::{method, prelude::*, DataTypeFunctions, Error, RClass, Ruby, TypedData, Value};
use rb_sys::VALUE;

use crate::text::VerifiedText;
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{ctx_budget, ResolverCall, XPathValue};
use crate::xpath::ctx::{Backend, OwnedContext};
use crate::xpath::limits::budget_sink;
use crate::xpath::msg::{
    ErrSink, Error as XPathError, Reported, XP_ERR_LIMIT, XP_ERR_OOM, XP_ERR_RUNTIME, XP_ERR_SYNTAX,
};
use crate::xpath::own::OwnedVal;
use crate::xpath::value::{NodeSet, TextSlot, Val, ValRef};

use super::abi::{
    doc_parsed, error_class, html_node_unwrap, is_kind_of, keepalive_document, node_raw,
    node_set_new, node_set_push, parsed_xml_doc, ruby_verified_text, xml_node_unwrap, RubyText,
    CLASS_NODE, CLASS_NODE_SET, CLASS_XML_DOCUMENT, MOD_HTML_NODE_METHODS,
};

/// An `XPathContext` is typically reused to run the same handful of expressions
/// many times, so each is parsed once and the AST re-evaluated (the evaluator
/// resets the per-eval counters and keeps its per-evaluate state off the AST,
/// so a cached AST is safely reusable). Bounded, so a context fed unbounded
/// distinct expressions cannot grow without limit.
const AST_CACHE_MAX: usize = 1024;

/// Upper bound on handler arguments, matching the engine's default
/// `max_function_args`. The resolver refuses any call above it, so the fixed
/// argv array below cannot overflow however the limit is tuned - and the stack
/// use stays independent of the runtime argument count.
const HANDLER_MAX_ARGS: usize = 64;

/// `DocKind`.
const DOC_XML: u32 = 1;

/// The engine context. Opaque here while C held it; now the real type.
use crate::xpath::ctx::Context as Ctx;

pub use crate::bridge::string::ruby_exception_message;
pub use crate::bridge::string::ruby_try_verified_text;
pub use crate::dom_adapter::dom_index::parsed_dom_index_build;
pub use crate::dom_adapter::dom_index::parsed_element_index;
pub use crate::dom_adapter::post_parse::parsed_kind;
pub use crate::init::CLASS_XPATH_CONTEXT;
pub use crate::init::EXC_XPATH_LIMIT_EXCEEDED;
pub use crate::init::EXC_XPATH_SYNTAX_ERROR;
pub use crate::xpath::ctx::ctx_is_evaluating;
pub use crate::xpath::ctx::ctx_limits;
pub use crate::xpath::ctx::ctx_set_context_node;
pub use crate::xpath::ctx::ctx_set_unprefixed_lax;
pub use crate::xpath::ctx::xpath_context_set_user_data;
pub use crate::xpath::ctx::xpath_register_ns;
pub use crate::xpath::ctx::xpath_register_variable_string;
pub use crate::xpath::ctx::xpath_set_func_resolver;
use crate::xpath::ctx::{evaluate, evaluate_first};
pub use crate::xpath::runtime_abi::nodeset_clear;
pub use crate::xpath::runtime_abi::nodeset_init;
pub use crate::xpath::runtime_abi::nodeset_push;
pub use crate::xpath::runtime_abi::val_set_borrowed_text_copy;

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/// An engine error as the Ruby exception it maps to.
///
/// Returned rather than raised: `rb_raise` longjmps past every Rust destructor
/// on the way (see `glue/mod.rs`), so each caller hands this back as `Err` and
/// magnus raises once its frames - and the context they own - are gone.
pub(crate) unsafe fn xpath_error(err: &XPathError) -> Error {
    let class = match err.status {
        XP_ERR_SYNTAX => EXC_XPATH_SYNTAX_ERROR,
        XP_ERR_LIMIT => EXC_XPATH_LIMIT_EXCEEDED,
        _ => error_class().as_raw(),
    };
    let msg =
        rb_sys::rb_utf8_str_new_cstr(err.message().unwrap_or(c"XPath evaluation failed").as_ptr());
    let exc = rb_sys::rb_exc_new_str(class, msg);
    match magnus::Exception::from_value(Value::from_raw(exc)) {
        Some(e) => Error::from(e),
        None => Error::new(error_class(), "XPath evaluation failed"),
    }
}

/// An evaluation result as a Ruby object. Converting consumes the value, which
/// frees its node-set array or string; `document` is the keepalive for a
/// node-set.
///
/// The conversion allocates - the NodeSet, its array, a String - and any of
/// those can raise `NoMemoryError`. So it runs under `protect`, reading `v` by
/// reference: a raise comes back as `Err`, and `v` is still freed on the way
/// out rather than skipped by the longjmp. One `protect` per result, not per
/// node.
///
/// Shared with the XML query glue, like [`xpath_error`].
pub(crate) unsafe fn value_to_ruby(v: XPathValue, document: Value) -> Result<Value, Error> {
    let converted = magnus::rb_sys::protect(|| match &v {
        XPathValue::NodeSet(set) => {
            let rb = node_set_new(document.as_raw());
            for &n in set.as_slice() {
                node_set_push(rb, n);
            }
            rb
        }
        XPathValue::String(t) => {
            let s = t.as_slice();
            rb_sys::rb_utf8_str_new(s.as_ptr() as *const c_char, s.len() as core::ffi::c_long)
        }
        XPathValue::Number(d) => rb_sys::rb_float_new(*d),
        XPathValue::Boolean(true) => rb_sys::Qtrue as VALUE,
        XPathValue::Boolean(false) => rb_sys::Qfalse as VALUE,
    });
    drop(v);
    Ok(Value::from_raw(converted?))
}

/// An engine string as a UTF-8 Ruby String. A NULL pointer is `""`.
unsafe fn owned_text_to_str(t: TextSlot) -> VALUE {
    let p = if t.is_absent() {
        c"".as_ptr()
    } else {
        t.as_ptr()
    };
    let n = if t.is_absent() { 0 } else { t.len() };
    rb_sys::rb_utf8_str_new(p, n as core::ffi::c_long)
}

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

struct Inner {
    /* Fields drop in declaration order, so the cached ASTs go before the
     * context they were parsed under. */
    cache: AstCache,
    ctx: OwnedContext,
}

/* SAFETY: every access holds the GVL, which serialises Ruby threads. magnus's
 * TypedData needs the bound because a wrapped value may be freed on whichever
 * thread runs the GC - still under the GVL. */
unsafe impl Send for Inner {}

/// `Makiri::XPathContext`.
///
/// `document` never changes and so is a plain field. `node` DOES change (via
/// `#node=`) and is also marked, which is why it is a `Cell` rather than living
/// in the `RefCell`: `mark` must reach both on every GC, and a GC can land while
/// a mutable borrow is outstanding - the mistake that had to be fixed in
/// `glue::node_set`.
#[derive(TypedData)]
#[magnus(class = "Makiri::XPathContext", mark, size, free_immediately)]
struct XPathCtx {
    /// Keepalive: owns the DOM arena.
    document: Opaque<Value>,
    /// Keepalive: the context node's wrapper.
    node: Cell<Opaque<Value>>,
    inner: RefCell<Inner>,
}

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
    fn borrow(&self) -> Result<core::cell::RefMut<'_, Inner>, Error> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| Error::new(unsafe { error_class() }, "XPath context is already in use"))
    }

    /// The native context pointer, read under a borrow that is released before
    /// it is used. Nothing frees the context while this object is alive, so the
    /// pointer stays valid - and not holding the borrow is what lets a handler
    /// re-enter (see `ctx_evaluate`).
    fn ctx(&self) -> Result<*mut Ctx, Error> {
        Ok(self.borrow()?.ctx.as_ptr())
    }
}

/* ------------------------------------------------------------------ */
/* context construction                                               */
/* ------------------------------------------------------------------ */

/// The three symbols the keyword check compares against, interned once.
/// `to_symbol` is a lookup, and this runs on the per-call path.
fn kw_symbols() -> (VALUE, VALUE, VALUE) {
    static SYMS: OnceLock<(VALUE, VALUE, VALUE)> = OnceLock::new();
    *SYMS.get_or_init(|| {
        // SAFETY: every caller is a Ruby method entered with the GVL. Symbols
        // are interned once and are immortal for the Ruby VM's lifetime.
        let sym = |s: &core::ffi::CStr| unsafe { rb_sys::rb_id2sym(rb_sys::rb_intern(s.as_ptr())) };
        (sym(c"namespace_matching"), sym(c"strict"), sym(c"lax"))
    })
}

/// Resolve the `namespace_matching:` keyword to the unprefixed-lax flag.
///
/// `:strict` (the default) resolves an unprefixed name test in the HTML
/// namespace, which is what browsers do; `:lax` makes it namespace-agnostic.
fn ns_matching_lax(ruby: &Ruby, opts: magnus::RHash) -> Result<c_int, Error> {
    if opts.is_empty() {
        return Ok(0);
    }
    let (key, strict, lax) = kw_symbols();
    let Some(v) = opts.get(unsafe { Value::from_raw(key) }) else {
        return Ok(0);
    };
    if v.is_nil() || v.as_raw() == strict {
        return Ok(0);
    }
    if v.as_raw() == lax {
        return Ok(1);
    }
    Err(Error::new(
        ruby.exception_arg_error(),
        format!(
            "namespace_matching: must be :strict or :lax, got {}",
            v.inspect()
        ),
    ))
}

/// Build a native context bound to `rb_node`'s document, with `rb_node` as the
/// context node.
///
/// The HTML branch builds the attr->owner index up front, so the engine's
/// parent and ancestor axes and its document-order sort see attribute owners,
/// and hands over the element index so `//tag` is answered without a tree walk.
/// The XML branch needs neither: the custom node links attributes to their owner
/// directly, and `//tag` falls back to a walk.
pub(crate) unsafe fn context_for(rb_node: Value, document: Value) -> Result<OwnedContext, Error> {
    let parsed = doc_parsed(document.as_raw())?;

    if parsed_kind(parsed) == DOC_XML {
        let xdoc = parsed_xml_doc(parsed);
        if xdoc.is_null() {
            return Err(Error::new(error_class(), "XPath context with no document"));
        }
        /* `ctx.doc` is the STORAGE (the Document); the context NODE is the
         * document node for a Document receiver, else the node itself. */
        let cnode = if is_kind_of(rb_node, CLASS_XML_DOCUMENT) {
            (*(xdoc as *mut crate::xml::model::Doc))
                .doc_node()
                .to_token() as *mut c_void
        } else {
            xml_node_unwrap(rb_node.as_raw())?
        };
        let Some(xctx) = OwnedContext::new(xdoc, cnode, Backend::Xml) else {
            return Err(Error::new(
                error_class(),
                "failed to allocate XPath context",
            ));
        };
        return Ok(xctx);
    }

    let node = html_node_unwrap(rb_node.as_raw())?;
    let doc = crate::glue::abi::html_doc_unwrap(document.as_raw())? as *mut c_void;
    if !parsed_dom_index_build(parsed) {
        return Err(Error::new(
            error_class(),
            "failed to build attribute index for XPath",
        ));
    }
    /* The element index is borrowed: it lives on the parsed document, which
     * outlives this context. */
    let Some(ctx) = OwnedContext::new(
        doc,
        node as *mut c_void,
        Backend::Html {
            index: parsed_element_index(parsed),
        },
    ) else {
        return Err(Error::new(
            error_class(),
            "failed to allocate XPath context",
        ));
    };
    Ok(ctx)
}

/// `XPathContext.new(node, namespace_matching: :strict)`.
fn ctx_s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let rb_node = a.required.0;
    let lax = ns_matching_lax(ruby, a.keywords)?;

    if !unsafe { is_kind_of(rb_node, CLASS_NODE) } {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::Node",
        ));
    }
    let document = unsafe { Value::from_raw(keepalive_document(rb_node.as_raw())?) };
    let ctx = unsafe { context_for(rb_node, document)? };
    unsafe { ctx_set_unprefixed_lax(ctx.as_ptr(), lax) };

    let obj = ruby
        .wrap(XPathCtx {
            document: document.into(),
            node: Cell::new(rb_node.into()),
            inner: RefCell::new(Inner {
                cache: AstCache(HashMap::new()),
                ctx,
            }),
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
    if !unsafe { is_kind_of(rb_node, CLASS_NODE) } {
        return Err(Error::new(
            ruby.exception_type_error(),
            "expected a Makiri::Node",
        ));
    }
    let ctx = rb_self.ctx()?;
    unsafe {
        if ctx_is_evaluating(ctx) != 0 {
            return Err(Error::new(
                error_class(),
                "cannot change the context node while evaluating (re-entrant mutation from a handler)",
            ));
        }
        if keepalive_document(rb_node.as_raw())? != ruby.get_inner(rb_self.document).as_raw() {
            return Err(Error::new(
                error_class(),
                "context node must belong to the same document",
            ));
        }
        rb_self.node.set(rb_node.into()); /* keepalive; marked above */
        /* Same-document is verified, so rb_node is the context's representation
         * and the engine - monomorphized per kind - takes the raw pointer. */
        ctx_set_context_node(ctx, node_raw(rb_node.as_raw())?);
    }
    Ok(rb_node)
}

/* ------------------------------------------------------------------ */
/* the custom-function handler bridge                                 */
/* ------------------------------------------------------------------ */

/* When an expression calls a function the engine does not know, it delegates to
 * a resolver. The one installed here dispatches to a Ruby handler object - the
 * method name is the XPath local name with '-' mapped to '_' - converting
 * arguments and the return value between engine and Ruby values. The call runs
 * under rb_protect, so a Ruby exception becomes a clean engine error rather than
 * a longjmp through the evaluator's C stack. */

struct Bridge {
    handler: VALUE,
    /// Keepalive, and the document node-set arguments are wrapped under.
    document: VALUE,
}

/// engine value -> Ruby.
unsafe fn arg_to_ruby(b: &Bridge, v: &Val) -> VALUE {
    match v.get() {
        ValRef::NodeSet(ns) => {
            let set = node_set_new(b.document);
            for i in 0..ns.count {
                node_set_push(set, *ns.items.add(i));
            }
            set
        }
        ValRef::String(t) => owned_text_to_str(t),
        ValRef::Number(d) => rb_sys::rb_float_new(d),
        ValRef::Boolean(true) => rb_sys::Qtrue as VALUE,
        ValRef::Boolean(false) => rb_sys::Qfalse as VALUE,
    }
}

/// Validate a handler-returned node and push it into the result node-set.
///
/// The same-document check compares the node's keepalive Document VALUE against
/// the context's, NOT the HTML-only owner_document field - so it is correct for
/// an XML node too, whose pointer is an arena node rather than a Lexbor one. A
/// node from another document fails closed.
unsafe fn push_result_node(
    ctx: *mut Ctx,
    document: VALUE,
    rb_node: VALUE,
    set: *mut NodeSet,
    err: &mut ErrBuf,
) -> bool {
    let Ok(node_document) = keepalive_document(rb_node) else {
        err.set("handler returned an unusable node");
        return false;
    };
    if node_document != document {
        err.set("handler returned a node from a different document");
        return false;
    }
    /* Same-document is checked above, so this is a node of the context's kind. */
    let Ok(n) = node_raw(rb_node) else {
        err.set("handler returned an unusable node");
        return false;
    };
    if nodeset_push(set, n, ctx_budget(ctx)).is_err() {
        err.set("out of memory building handler result");
        return false;
    }
    true
}

/// A fixed message buffer, so reporting a conversion failure allocates nothing
/// on a path that is already failing.
struct ErrBuf {
    buf: [u8; 200],
    len: usize,
}

impl ErrBuf {
    fn new() -> Self {
        ErrBuf {
            buf: [0; 200],
            len: 0,
        }
    }
    fn set(&mut self, msg: &str) {
        let b = msg.as_bytes();
        self.len = b.len().min(self.buf.len() - 1);
        self.buf[..self.len].copy_from_slice(&b[..self.len]);
        self.buf[self.len] = 0;
    }
    fn set_fmt(&mut self, args: core::fmt::Arguments<'_>) {
        use core::fmt::Write;
        struct W<'a>(&'a mut ErrBuf);
        impl core::fmt::Write for W<'_> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let room = self.0.buf.len() - 1 - self.0.len;
                let n = s.len().min(room);
                self.0.buf[self.0.len..self.0.len + n].copy_from_slice(&s.as_bytes()[..n]);
                self.0.len += n;
                self.0.buf[self.0.len] = 0;
                Ok(())
            }
        }
        self.len = 0;
        let _ = W(self).write_fmt(args);
    }
    fn as_ptr(&self) -> *const c_char {
        self.buf.as_ptr() as *const c_char
    }
}

/// Ruby return value -> engine value.
unsafe fn ruby_to_out(
    ctx: *mut Ctx,
    document: VALUE,
    r: VALUE,
    out: *mut Val,
    err: &mut ErrBuf,
) -> bool {
    let rv = Value::from_raw(r);
    if r == rb_sys::Qtrue as VALUE || r == rb_sys::Qfalse as VALUE {
        *out = Val::boolean(r == rb_sys::Qtrue as VALUE);
        return true;
    }
    let ruby = Ruby::get_unchecked();
    if is_numeric(&ruby, rv) {
        let Ok(f) = f64::try_convert(rv) else {
            err.set("handler returned a number that could not be read");
            return false;
        };
        *out = Val::number(f);
        return true;
    }
    let is_node = is_kind_of(rv, CLASS_NODE);
    if is_node || is_kind_of(rv, CLASS_NODE_SET) {
        let mut set = NodeSet::EMPTY;
        if is_node {
            if !push_result_node(ctx, document, r, &mut set, err) {
                nodeset_clear(&mut set);
                return false;
            }
        } else {
            let Ok(n) = rv.funcall::<_, _, i64>("length", ()) else {
                err.set("handler result could not be read");
                return false;
            };
            for i in 0..n {
                let Ok(node) = rv.funcall::<_, _, Value>("[]", (i,)) else {
                    continue;
                };
                if !is_kind_of(node, CLASS_NODE) {
                    continue;
                }
                if !push_result_node(ctx, document, node.as_raw(), &mut set, err) {
                    nodeset_clear(&mut set);
                    return false;
                }
            }
        }
        *out = Val::nodeset(set);
        return true;
    }

    /* nil and everything else: coerce to a string (nil -> ""). */
    if rv.is_nil() {
        if val_set_borrowed_text_copy(out, VerifiedText::empty().into(), ErrSink::silent(), None)
            != 0
        {
            err.set("out of memory converting handler result");
            return false;
        }
        return true;
    }
    let Ok(sv) = rv.funcall::<_, _, Value>("to_s", ()) else {
        err.set("handler result could not be converted to a string");
        return false;
    };
    let vv = match ruby_try_verified_text(sv.as_raw(), (*ctx_limits(ctx)).max_string_bytes) {
        Ok(vv) => vv,
        Err(reason) => {
            let reason = reason.to_string_lossy();
            err.set_fmt(format_args!("handler returned an invalid string: {reason}"));
            return false;
        }
    };
    let rc = val_set_borrowed_text_copy(
        out,
        unsafe { vv.as_verified() }.into(),
        ErrSink::silent(),
        None,
    );
    if rc != 0 || !matches!((*out).get(), ValRef::String(t) if t.is_present()) {
        err.set("out of memory converting handler result");
        return false;
    }
    true
}

/// Integer and Float both become an XPath number; a String that happens to look
/// numeric does not - the C tested the types, not convertibility.
fn is_numeric(ruby: &Ruby, v: Value) -> bool {
    v.is_kind_of(ruby.class_integer()) || v.is_kind_of(ruby.class_float())
}

/// Everything the protected call needs. `argv` is a fixed array, so the stack
/// use does not depend on the runtime argument count.
struct HandlerCall {
    bridge: *const Bridge,
    ctx: *mut Ctx,
    method: rb_sys::ID,
    args: *const Val,
    nargs: usize,
    out: *mut Val,
    ok: bool,
    err: ErrBuf,
    argv: [VALUE; HANDLER_MAX_ARGS],
}

/// Runs under `rb_protect`: build the Ruby arguments, invoke the handler,
/// convert the result.
unsafe extern "C" fn handler_call_body(p: VALUE) -> VALUE {
    let c = &mut *(p as *mut HandlerCall);
    for i in 0..c.nargs {
        c.argv[i] = arg_to_ruby(&*c.bridge, &*c.args.add(i));
    }
    let r = rb_sys::rb_funcallv(
        (*c.bridge).handler,
        c.method,
        c.nargs as c_int,
        c.argv.as_ptr(),
    );
    c.ok = ruby_to_out(c.ctx, (*c.bridge).document, r, c.out, &mut c.err);
    rb_sys::Qnil as VALUE
}

/// The engine's resolver hook: the Ruby handler's method for the call, or
/// `Ok(None)` when the handler has no such method and the engine reports the
/// function unknown.
unsafe fn handler_resolver(
    user_data: *mut c_void,
    ctx: *mut Ctx,
    call: &ResolverCall<'_>,
) -> Result<Option<OwnedVal>, Reported> {
    let err = budget_sink(ctx_budget(ctx));
    if user_data.is_null() {
        return Ok(None);
    }
    let bridge = &*(user_data as *const Bridge);
    if bridge.handler == rb_sys::Qnil as VALUE {
        return Ok(None);
    }

    /* The method name: XPath uses '-', Ruby uses '_'. The buffer starts zeroed,
     * so the copy stays NUL-terminated. */
    let mut name = [0u8; 128];
    let n = call.local.len();
    if n >= name.len() {
        return Ok(None); /* too long to map to a Ruby method name */
    }
    for (dst, &b) in name.iter_mut().zip(call.local) {
        *dst = if b == b'-' { b'_' } else { b };
    }

    let method = rb_sys::rb_intern(name.as_ptr() as *const c_char);
    if rb_sys::rb_respond_to(bridge.handler, method) == 0 {
        return Ok(None); /* let the engine raise "unknown function" */
    }

    if call.args.len() > HANDLER_MAX_ARGS {
        return Err(crate::err_setf!(
            err,
            XP_ERR_RUNTIME,
            "handler function '{}' called with too many arguments ({} > {})",
            core::str::from_utf8_unchecked(&name[..n]),
            call.args.len(),
            HANDLER_MAX_ARGS
        ));
    }

    let mut out = Val::EMPTY;
    let mut state_of_call = HandlerCall {
        bridge,
        ctx,
        method,
        args: call.args.as_ptr(),
        nargs: call.args.len(),
        out: &mut out,
        ok: true,
        err: ErrBuf::new(),
        argv: [rb_sys::Qnil as VALUE; HANDLER_MAX_ARGS],
    };

    let mut state: c_int = 0;
    rb_sys::rb_protect(
        Some(handler_call_body),
        &mut state_of_call as *mut HandlerCall as VALUE,
        &mut state,
    );
    /* Whatever the handler produced is owned from here, so every failure below
     * frees it. */
    let out = OwnedVal::from(out);
    if state != 0 {
        let exc = rb_sys::rb_errinfo();
        rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
        // c_char, not i8: it is signed on aarch64-darwin and UNSIGNED on
        // aarch64-linux, so spelling the element type concretely compiles on
        // one release platform and fails on another.
        let mut msg = [0 as c_char; 200];
        ruby_exception_message(exc, msg.as_mut_ptr(), msg.len());
        return Err(crate::err_setf!(
            err,
            XP_ERR_RUNTIME,
            "handler raised: {}",
            core::ffi::CStr::from_ptr(msg.as_ptr()).to_string_lossy()
        ));
    }
    if !state_of_call.ok {
        return Err(crate::xpath::msg::err_set(
            err,
            XP_ERR_RUNTIME,
            core::ffi::CStr::from_ptr(state_of_call.err.as_ptr()),
        ));
    }
    Ok(Some(out))
}

/* ------------------------------------------------------------------ */
/* evaluate                                                           */
/* ------------------------------------------------------------------ */

/// The compiled AST for `expr`, parsing and caching it on first use.
///
/// Returns a pointer to the AST plus its owner when it could not be cached. A
/// cached AST lives as long as the context (see [`AstCache`]).
unsafe fn cached_ast(d: &mut Inner, expr: RubyText) -> Option<(*const Ast, Option<Box<Ast>>)> {
    let key = expr.bytes();
    if let Some(ast) = d.cache.0.get(key) {
        return Some((&**ast as *const Ast, None));
    }

    let budget = ctx_budget(d.ctx.as_ptr());
    (*budget).limits.ast_nodes = 0;
    let ast = crate::xpath::parse::parse_owned(unsafe { expr.as_verified() }, budget).ok()?;
    if d.cache.0.len() >= AST_CACHE_MAX || d.cache.0.mkr_reserve(1).is_err() {
        return Some((&*ast as *const Ast, Some(ast)));
    }
    let Some(owned_key) = try_to_boxed_slice(key) else {
        return Some((&*ast as *const Ast, Some(ast)));
    };
    if d.cache.0.mkr_insert(owned_key, ast).is_err() {
        crate::xpath::msg::err_set(
            budget_sink(budget),
            XP_ERR_OOM,
            c"out of memory caching XPath expression",
        );
        return None;
    }
    let ast = d.cache.0.get(key).expect("inserted AST");
    Some((&**ast as *const Ast, None))
}

/// Install the handler bridge for one evaluation, and take it back off.
///
/// It has to come off: the bridge lives on the caller's stack, and the context
/// outlives the call.
struct InstalledHandler {
    ctx: *mut Ctx,
    installed: bool,
}

impl InstalledHandler {
    unsafe fn new(ctx: *mut Ctx, bridge: *const Bridge, handler: VALUE) -> Self {
        let installed = handler != rb_sys::Qnil as VALUE;
        if installed {
            xpath_context_set_user_data(ctx, bridge as *mut c_void);
            xpath_set_func_resolver(ctx, Some(handler_resolver));
        }
        InstalledHandler { ctx, installed }
    }
}

impl Drop for InstalledHandler {
    fn drop(&mut self) {
        if self.installed {
            // SAFETY: undoes exactly what new() did.
            unsafe {
                xpath_set_func_resolver(self.ctx, None);
                xpath_context_set_user_data(self.ctx, core::ptr::null_mut());
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* the query path every entry point shares                            */
/* ------------------------------------------------------------------ */
/* `Node#xpath` / `#at_xpath` for both representations, the XML `#css` family
 * and `XPathContext#evaluate` all run parse -> evaluate -> convert through
 * these three; they differ only in how the context is built and who owns it. */

/// Parse `expr` for one query under `ctx`. The AST-node budget is per query, so
/// it is reset first, and a failure is that budget's error as the exception.
pub(crate) unsafe fn parse_query(ctx: *mut Ctx, expr: Value) -> Result<Box<Ast>, Error> {
    let ev = ruby_verified_text(expr.as_raw(), c"XPath expression".as_ptr())?;
    let budget = ctx_budget(ctx);
    (*budget).limits.ast_nodes = 0;
    let parsed = crate::xpath::parse::parse_owned(ev.as_verified(), budget);
    /* No borrowed bytes across the exception's allocation. */
    drop(ev);
    parsed.map_err(|_| xpath_error(&(*budget).take_error()))
}

/// Evaluate `ast` under `ctx`, with `handler` (nil for none) answering unknown
/// functions for this evaluation only. `first_only` takes the `at_xpath` fast
/// path.
pub(crate) unsafe fn evaluate_query(
    ctx: *mut Ctx,
    ast: &Ast,
    handler: Value,
    document: Value,
    first_only: bool,
) -> Result<XPathValue, Error> {
    /* A handler runs Ruby mid-walk, so for as long as one can, the document
     * refuses to be changed. Declared first, so it is released last. */
    let _reading = if handler.is_nil() {
        None
    } else {
        Some(crate::glue::doc::DocumentEvaluation::enter(
            document.as_raw(),
        )?)
    };
    let bridge = Bridge {
        handler: handler.as_raw(),
        document: document.as_raw(),
    };
    let installed = InstalledHandler::new(ctx, &bridge, handler.as_raw());
    let result = if first_only {
        evaluate_first(ctx, ast)
    } else {
        evaluate(ctx, ast)
    };
    drop(installed);
    result.map_err(|error| xpath_error(&error))
}

/// A query's value as Ruby, and for `at_xpath` the first node of a node-set.
///
/// Callers free the AST and any context they own BEFORE this: the value owns
/// its data and references neither.
pub(crate) unsafe fn query_result(
    value: XPathValue,
    document: Value,
    first_only: bool,
) -> Result<Value, Error> {
    let result = value_to_ruby(value, document)?;
    if first_only && is_kind_of(result, CLASS_NODE_SET) {
        return result.funcall("first", ());
    }
    Ok(result)
}

fn ctx_evaluate(ruby: &Ruby, rb_self: &XPathCtx, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let expr = a.required.0;
    let handler = a.optional.0.unwrap_or(ruby.qnil().as_value());
    let document = ruby.get_inner(rb_self.document);

    /* The borrow is taken for the cache lookup ONLY, and released before the
     * evaluation. A handler called mid-walk re-enters this object - to register
     * a namespace, to rebind the node, or to evaluate again on the same context
     * - and re-entrancy is governed by the engine's own `is_evaluating` flag,
     * which reports the specific refusal (and permits a nested evaluate). Holding
     * the borrow across the walk would turn all four into one generic "already in
     * use", which is how the handler specs first caught this. */
    let (ctx, ast, owned) = unsafe {
        /* Verify BEFORE borrowing: coercing the expression can run Ruby (`to_s`),
         * which may re-enter this context, and a borrow held across that would
         * turn the re-entry into "already in use". */
        let ev = ruby_verified_text(expr.as_raw(), c"XPath expression".as_ptr())?;
        let mut d = rb_self.borrow()?;
        let parsed = cached_ast(&mut d, ev);
        let ctx = d.ctx.as_ptr();
        /* Release the borrow before building the exception: that allocates, and
         * a NoMemoryError there would longjmp past the RefMut. */
        drop(d);
        match parsed {
            Some((ast, owned)) => (ctx, ast, owned),
            None => return Err(xpath_error(&(*ctx_budget(ctx)).take_error())),
        }
    };

    unsafe {
        /* A cached AST outlives this call: the context is live (it is
         * `rb_self`), and its cache frees nothing before the context goes. */
        let value = evaluate_query(ctx, &*ast, handler, document, false);
        drop(owned);
        query_result(value?, document, false)
    }
}

fn ctx_register_ns(rb_self: &XPathCtx, prefix: Value, uri: Value) -> Result<Value, Error> {
    let ctx = rb_self.ctx()?;
    unsafe {
        if ctx_is_evaluating(ctx) != 0 {
            return Err(Error::new(
                error_class(),
                "cannot register a namespace while evaluating (re-entrant mutation from a handler)",
            ));
        }
        let pv = ruby_verified_text(prefix.as_raw(), c"namespace prefix".as_ptr())?;
        let uv = ruby_verified_text(uri.as_raw(), c"namespace URI".as_ptr())?;
        let rc = xpath_register_ns(ctx, pv.as_verified(), uv.as_verified()); /* copies both */
        if rc != 0 {
            return Err(Error::new(error_class(), "failed to register namespace"));
        }
    }
    Ok(rb_self_value())
}

/// magnus hands a method a `&XPathCtx`, not the object; the receiver is
/// recovered from the frame for the `self`-returning registrars.
fn rb_self_value() -> Value {
    unsafe { Value::from_raw(rb_sys::rb_current_receiver()) }
}

fn ctx_register_variable(rb_self: &XPathCtx, name: Value, value: Value) -> Result<Value, Error> {
    let ctx = rb_self.ctx()?;
    unsafe {
        if ctx_is_evaluating(ctx) != 0 {
            return Err(Error::new(
                error_class(),
                "cannot register a variable while evaluating (re-entrant mutation from a handler)",
            ));
        }
        /* Coerce the value FIRST - to_s allocates, which is a GC point - so no
         * borrowed name bytes are held across it. The value then gets the
         * stricter engine-string check, which adds the byte cap on top of the
         * no-NUL / valid-UTF-8 contract. */
        let sv: Value = value.funcall("to_s", ())?;
        let nv = ruby_verified_text(name.as_raw(), c"variable name".as_ptr())?;
        let vv = match ruby_try_verified_text(sv.as_raw(), (*ctx_limits(ctx)).max_string_bytes) {
            Ok(vv) => vv,
            Err(reason) => {
                return Err(Error::new(
                    error_class(),
                    format!("invalid variable value: {}", reason.to_string_lossy()),
                ));
            }
        };
        let rc = xpath_register_variable_string(ctx, nv.as_verified(), vv.as_verified()); /* copies both */
        if rc != 0 {
            return Err(Error::new(error_class(), "failed to register variable"));
        }
    }
    Ok(rb_self_value())
}

/* ------------------------------------------------------------------ */
/* Node#xpath / Node#at_xpath                                         */
/* ------------------------------------------------------------------ */

/// A throwaway context per call, so `Node#xpath` caches nothing;
/// `Makiri::XPathContext` is what a caller reaches for when many queries share
/// one namespace set and one set of compiled expressions.
fn node_xpath_run(
    rb_self: Value,
    expr: Value,
    handler: Value,
    lax: c_int,
    first_only: bool,
) -> Result<Value, Error> {
    unsafe {
        let document = Value::from_raw(keepalive_document(rb_self.as_raw())?);
        let ctx = context_for(rb_self, document)?;
        ctx_set_unprefixed_lax(ctx.as_ptr(), lax);
        let ast = parse_query(ctx.as_ptr(), expr)?;
        let value = evaluate_query(ctx.as_ptr(), &ast, handler, document, first_only);
        drop(ast);
        drop(ctx);
        query_result(value?, document, first_only)
    }
}

/// `(expression, handler, lax)` from the argument list.
///
/// The one-argument call - `node.xpath(expr)`, much the commonest - is answered
/// before `scan_args` runs at all. That is not a micro-optimisation: `scan_args`
/// with a keyword type allocates an empty Hash even when no keywords were
/// passed, and `at_xpath` spends about 650ns per call in total, so the
/// allocation and the symbol lookups behind it measured ~32% of it.
fn scan_query_args(ruby: &Ruby, args: &[Value]) -> Result<(Value, Value, c_int), Error> {
    if args.len() == 1 {
        return Ok((args[0], ruby.qnil().as_value(), 0));
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
    let (expr, handler, lax) = scan_query_args(ruby, args)?;
    node_xpath_run(rb_self, expr, handler, lax, false)
}

/// The first matching node for a node-set result, or the scalar otherwise.
fn node_at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let (expr, handler, lax) = scan_query_args(ruby, args)?;
    node_xpath_run(rb_self, expr, handler, lax, true)
}

/// # Safety
/// From `Init_makiri`.
pub unsafe extern "C" fn init_xpath() {
    let klass = RClass::from_value(Value::from_raw(CLASS_XPATH_CONTEXT))
        .expect("Makiri::XPathContext is a Class");
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

    let m = magnus::RModule::from_value(Value::from_raw(MOD_HTML_NODE_METHODS))
        .expect("Makiri::HTML::NodeMethods");
    m.define_method("xpath", method!(node_xpath, -1))
        .expect("#xpath");
    m.define_method("at_xpath", method!(node_at_xpath, -1))
        .expect("#at_xpath");
}
