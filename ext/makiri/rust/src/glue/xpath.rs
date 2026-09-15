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

use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Backend, Context, ContextError, Resolver};
use crate::xpath::ctx::{ResolverCall, XPathValue};
use crate::xpath::limits::Budget;
use crate::xpath::msg::{
    Error as XPathError, Reported, XP_ERR_LIMIT, XP_ERR_OOM, XP_ERR_RUNTIME, XP_ERR_SYNTAX,
};
use crate::xpath::value::{NodeSet, Text, Val, ValRef};

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

pub use crate::bridge::string::ruby_exception_message;
pub use crate::bridge::string::ruby_try_verified_text;
pub use crate::init::CLASS_XPATH_CONTEXT;
pub use crate::init::EXC_XPATH_LIMIT_EXCEEDED;
pub use crate::init::EXC_XPATH_SYNTAX_ERROR;

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/// An engine error as the Ruby exception it maps to.
///
/// Returned rather than raised: `rb_raise` longjmps past every Rust destructor
/// on the way (see `glue/mod.rs`), so each caller hands this back as `Err` and
/// magnus raises once its frames - and the context they own - are gone.
pub(crate) fn xpath_error(err: &XPathError) -> Error {
    let class = match err.status {
        XP_ERR_SYNTAX => EXC_XPATH_SYNTAX_ERROR.exception(),
        XP_ERR_LIMIT => EXC_XPATH_LIMIT_EXCEEDED.exception(),
        _ => error_class(),
    };
    let ruby = Ruby::get_with(class);
    /* The message's bytes as they are, tagged UTF-8 - not a lossy copy. */
    let bytes = err
        .message()
        .unwrap_or(c"XPath evaluation failed")
        .to_bytes();
    let msg = ruby.enc_str_new(bytes, ruby.utf8_encoding());
    match class.new_instance((msg,)) {
        Ok(e) => Error::from(e),
        Err(e) => e,
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
    ctx: Context<'static>,
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
            .map_err(|_| Error::new(error_class(), "XPath context is already in use"))
    }
}

/// A context's refusal as the exception it raises: `busy` when an evaluate is
/// running on it, `failed` otherwise.
fn refused(error: ContextError, busy: &'static str, failed: &'static str) -> Error {
    let msg = match error {
        ContextError::Evaluating => busy,
        ContextError::Failed => failed,
    };
    Error::new(error_class(), msg)
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
fn ns_matching_lax(ruby: &Ruby, opts: magnus::RHash) -> Result<bool, Error> {
    if opts.is_empty() {
        return Ok(false);
    }
    let (key, strict, lax) = kw_symbols();
    let Some(v) = opts.get(unsafe { Value::from_raw(key) }) else {
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

/// Build a native context bound to `rb_node`'s document, with `rb_node` as the
/// context node.
///
/// The HTML branch builds the attr->owner index up front, so the engine's
/// parent and ancestor axes and its document-order sort see attribute owners,
/// and hands over the element index so `//tag` is answered without a tree walk.
/// The XML branch needs neither: the custom node links attributes to their owner
/// directly, and its name index hangs off the document.
///
/// The context is `'static` because Ruby, not a Rust borrow, keeps the document
/// alive: the caller holds `document` for as long as the context lives. The
/// document does not change while an evaluate runs: without a handler no Ruby
/// runs, and with one [`Bridge`] holds the document's mutation guard.
pub(crate) unsafe fn context_for(
    rb_node: Value,
    document: Value,
) -> Result<Context<'static>, Error> {
    let parsed = doc_parsed(document)?;

    if (*parsed).is_xml() {
        let xdoc = parsed_xml_doc(parsed);
        if xdoc.is_null() {
            return Err(Error::new(error_class(), "XPath context with no document"));
        }
        /* `ctx.doc` is the STORAGE (the Document); the context NODE is the
         * document node for a Document receiver, else the node itself. */
        let cnode = if is_kind_of(rb_node, &CLASS_XML_DOCUMENT) {
            (*(xdoc as *mut crate::xml::model::Doc))
                .doc_node()
                .to_token() as *mut c_void
        } else {
            xml_node_unwrap(rb_node)?
        };
        let backend = Backend::Xml {
            doc: xdoc as *const crate::xml::model::Document,
        };
        return Ok(Context::new(backend, cnode));
    }

    let node = html_node_unwrap(rb_node)?;
    /* TypeError for a Document that is not HTML. */
    crate::glue::abi::html_doc_unwrap(document)?;
    /* Built up front, so an allocation failure raises here rather than on the
     * first evaluate. Each evaluate still reads the index afresh from the
     * handle, which rebuilds it after a mutation - the context must not keep
     * the one it saw here. */
    if (*parsed).dom_index().is_none() {
        return Err(Error::new(
            error_class(),
            "failed to build attribute index for XPath",
        ));
    }
    let backend = Backend::Html { parsed };
    Ok(Context::new(backend, node as *mut c_void))
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
    let mut ctx = unsafe { context_for(rb_node, document)? };
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
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, BUSY));
    }
    unsafe {
        if keepalive_document(rb_node)?.as_raw() != ruby.get_inner(rb_self.document).as_raw() {
            return Err(Error::new(
                error_class(),
                "context node must belong to the same document",
            ));
        }
        rb_self.node.set(rb_node.into()); /* keepalive; marked above */
        /* Same-document is verified, so rb_node is a node of the context's
         * document. */
        rb_self
            .ctx
            .set_context_node(node_raw(rb_node)?)
            .map_err(|e| refused(e, BUSY, BUSY))?;
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
    /// Every mutator on `document` refuses while this lives.
    _reading: crate::glue::doc::DocumentEvaluation,
}

// SAFETY: the bridge holds `document`'s evaluation guard for as long as it
// exists, so a handler cannot change the document mid-walk; and
// `push_result_node` admits only nodes whose document is `document`.
unsafe impl Resolver for Bridge {
    fn resolve(
        &self,
        budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<Val>, Reported> {
        // SAFETY: called by the engine mid-evaluate, under the GVL.
        unsafe { handler_resolver(self, budget, call) }
    }
}

/// engine value -> Ruby.
unsafe fn arg_to_ruby(b: &Bridge, v: &Val) -> VALUE {
    match v.get() {
        ValRef::NodeSet(ns) => {
            let set = node_set_new(b.document);
            for &n in ns.as_slice() {
                node_set_push(set, n);
            }
            set
        }
        ValRef::String(t) => {
            let s = t.as_slice();
            rb_sys::rb_utf8_str_new(s.as_ptr() as *const c_char, s.len() as core::ffi::c_long)
        }
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
    budget: &mut Budget,
    document: VALUE,
    rb_node: VALUE,
    set: &mut NodeSet,
    err: &mut ErrBuf,
) -> bool {
    let rb_node = Value::from_raw(rb_node);
    let Ok(node_document) = keepalive_document(rb_node) else {
        err.set("handler returned an unusable node");
        return false;
    };
    if node_document.as_raw() != document {
        err.set("handler returned a node from a different document");
        return false;
    }
    /* Same-document is checked above, so this is a node of the context's kind. */
    let Ok(n) = node_raw(rb_node) else {
        err.set("handler returned an unusable node");
        return false;
    };
    if set.push_token(n, budget).is_err() {
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
    budget: &mut Budget,
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
    let is_node = is_kind_of(rv, &CLASS_NODE);
    if is_node || is_kind_of(rv, &CLASS_NODE_SET) {
        let mut set = NodeSet::new();
        if is_node {
            if !push_result_node(budget, document, r, &mut set, err) {
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
                if !is_kind_of(node, &CLASS_NODE) {
                    continue;
                }
                if !push_result_node(budget, document, node.as_raw(), &mut set, err) {
                    return false;
                }
            }
        }
        *out = Val::nodeset(set);
        return true;
    }

    /* nil and everything else: coerce to a string (nil -> ""). */
    if rv.is_nil() {
        *out = Val::string(Text::default());
        return true;
    }
    let Ok(sv) = rv.funcall::<_, _, Value>("to_s", ()) else {
        err.set("handler result could not be converted to a string");
        return false;
    };
    let vv = match ruby_try_verified_text(sv.as_raw(), budget.limits.max_string_bytes) {
        Ok(vv) => vv,
        Err(reason) => {
            let reason = reason.to_string_lossy();
            err.set_fmt(format_args!("handler returned an invalid string: {reason}"));
            return false;
        }
    };
    let Some(text) = Text::try_copy(unsafe { vv.as_verified() }.as_bytes()) else {
        err.set("out of memory converting handler result");
        return false;
    };
    *out = Val::string(text);
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
    budget: *mut Budget,
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
    c.ok = ruby_to_out(&mut *c.budget, (*c.bridge).document, r, c.out, &mut c.err);
    rb_sys::Qnil as VALUE
}

/// The engine's resolver hook: the Ruby handler's method for the call, or
/// `Ok(None)` when the handler has no such method and the engine reports the
/// function unknown.
unsafe fn handler_resolver(
    bridge: &Bridge,
    budget: &mut Budget,
    call: &ResolverCall<'_>,
) -> Result<Option<Val>, Reported> {
    let err = budget.sink();
    let budget: *mut Budget = budget;
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

    let mut out = Val::default();
    let mut state_of_call = HandlerCall {
        bridge,
        budget,
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
    /* `out` owns whatever the handler produced, so every failure below frees
     * it. */
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
#[allow(clippy::result_large_err)]
unsafe fn cached_ast(
    cache: &mut AstCache,
    limits: crate::xpath::limits::Limits,
    expr: RubyText,
) -> Result<(*const Ast, Option<Box<Ast>>), crate::xpath::msg::Error> {
    let key = expr.bytes();
    if let Some(ast) = cache.0.get(key) {
        return Ok((&**ast as *const Ast, None));
    }

    /* Each parse charges a budget of its own, made from the context's caps. */
    let mut budget = Budget::with_limits(limits);
    let Ok(ast) = crate::xpath::parse::parse_owned(unsafe { expr.as_verified() }, &mut budget)
    else {
        return Err(budget.take_error());
    };
    if cache.0.len() >= AST_CACHE_MAX || cache.0.mkr_reserve(1).is_err() {
        return Ok((&*ast as *const Ast, Some(ast)));
    }
    let Some(owned_key) = try_to_boxed_slice(key) else {
        return Ok((&*ast as *const Ast, Some(ast)));
    };
    if cache.0.mkr_insert(owned_key, ast).is_err() {
        crate::xpath::msg::err_set(
            budget.sink(),
            XP_ERR_OOM,
            c"out of memory caching XPath expression",
        );
        return Err(budget.take_error());
    }
    let ast = cache.0.get(key).expect("inserted AST");
    Ok((&**ast as *const Ast, None))
}

/* ------------------------------------------------------------------ */
/* the query path every entry point shares                            */
/* ------------------------------------------------------------------ */
/* `Node#xpath` / `#at_xpath` for both representations, the XML `#css` family
 * and `XPathContext#evaluate` all run parse -> evaluate -> convert through
 * these three; they differ only in how the context is built and who owns it. */

/// Parse `expr` for one query under `ctx`'s caps, on a budget of the query's
/// own; a failure is that budget's error as the exception.
pub(crate) unsafe fn parse_query(ctx: &Context, expr: Value) -> Result<Box<Ast>, Error> {
    let ev = ruby_verified_text(expr, c"XPath expression")?;
    let mut budget = Budget::with_limits(ctx.limits());
    let parsed = crate::xpath::parse::parse_owned(ev.as_verified(), &mut budget);
    /* No borrowed bytes across the exception's allocation. */
    drop(ev);
    parsed.map_err(|_| xpath_error(&budget.take_error()))
}

/// Evaluate `ast` under `ctx`, with `handler` (nil for none) answering unknown
/// functions for this evaluation only. `first_only` takes the `at_xpath` fast
/// path.
pub(crate) unsafe fn evaluate_query(
    ctx: &Context,
    ast: &Ast,
    handler: Value,
    document: Value,
    first_only: bool,
) -> Result<XPathValue, Error> {
    /* A handler runs Ruby mid-walk, so for as long as one can, the document
     * refuses to be changed: the bridge holds that guard, and lives on this
     * frame for this evaluate alone. */
    let bridge = if handler.is_nil() {
        None
    } else {
        Some(Bridge {
            handler: handler.as_raw(),
            document: document.as_raw(),
            _reading: crate::glue::doc::DocumentEvaluation::enter(document)?,
        })
    };
    let resolver = bridge.as_ref().map(|b| b as &dyn Resolver);
    let result = if first_only {
        ctx.evaluate_first(ast, resolver)
    } else {
        ctx.evaluate(ast, resolver)
    };
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
    if first_only && is_kind_of(result, &CLASS_NODE_SET) {
        return result.funcall("first", ());
    }
    Ok(result)
}

fn ctx_evaluate(ruby: &Ruby, rb_self: &XPathCtx, args: &[Value]) -> Result<Value, Error> {
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
    let (ast, owned) = unsafe {
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

    unsafe {
        /* A cached AST outlives this call: the context is live (it is
         * `rb_self`), and its cache frees nothing before the context goes. */
        let value = evaluate_query(&rb_self.ctx, &*ast, handler, document, false);
        drop(owned);
        query_result(value?, document, false)
    }
}

fn ctx_register_ns(rb_self: &XPathCtx, prefix: Value, uri: Value) -> Result<Value, Error> {
    const BUSY: &str =
        "cannot register a namespace while evaluating (re-entrant mutation from a handler)";
    const FAILED: &str = "failed to register namespace";
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, FAILED));
    }
    unsafe {
        let pv = ruby_verified_text(prefix, c"namespace prefix")?;
        let uv = ruby_verified_text(uri, c"namespace URI")?;
        rb_self
            .ctx
            .register_ns(pv.as_verified().as_bytes(), uv.as_verified().as_bytes()) /* copies both */
            .map_err(|e| refused(e, BUSY, FAILED))?;
    }
    Ok(rb_self_value())
}

/// magnus hands a method a `&XPathCtx`, not the object; the receiver is
/// recovered from the frame for the `self`-returning registrars.
fn rb_self_value() -> Value {
    unsafe { Value::from_raw(rb_sys::rb_current_receiver()) }
}

fn ctx_register_variable(rb_self: &XPathCtx, name: Value, value: Value) -> Result<Value, Error> {
    const BUSY: &str =
        "cannot register a variable while evaluating (re-entrant mutation from a handler)";
    const FAILED: &str = "failed to register variable";
    if rb_self.ctx.is_evaluating() {
        return Err(refused(ContextError::Evaluating, BUSY, FAILED));
    }
    unsafe {
        /* Coerce the value FIRST - to_s allocates, which is a GC point - so no
         * borrowed name bytes are held across it. The value then gets the
         * stricter engine-string check, which adds the byte cap on top of the
         * no-NUL / valid-UTF-8 contract. */
        let sv: Value = value.funcall("to_s", ())?;
        let nv = ruby_verified_text(name, c"variable name")?;
        let vv = match ruby_try_verified_text(sv.as_raw(), rb_self.ctx.limits().max_string_bytes) {
            Ok(vv) => vv,
            Err(reason) => {
                return Err(Error::new(
                    error_class(),
                    format!("invalid variable value: {}", reason.to_string_lossy()),
                ));
            }
        };
        rb_self
            .ctx
            .register_variable(nv.as_verified().as_bytes(), vv.as_verified().as_bytes()) /* copies both */
            .map_err(|e| refused(e, BUSY, FAILED))?;
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
    lax: bool,
    first_only: bool,
) -> Result<Value, Error> {
    unsafe {
        let document = keepalive_document(rb_self)?;
        let mut ctx = context_for(rb_self, document)?;
        ctx.set_lax(lax);
        let ast = parse_query(&ctx, expr)?;
        let value = evaluate_query(&ctx, &ast, handler, document, first_only);
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
fn scan_query_args(ruby: &Ruby, args: &[Value]) -> Result<(Value, Value, bool), Error> {
    if args.len() == 1 {
        return Ok((args[0], ruby.qnil().as_value(), false));
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

    let m = magnus::RModule::from_value(MOD_HTML_NODE_METHODS.value())
        .expect("Makiri::HTML::NodeMethods");
    m.define_method("xpath", method!(node_xpath, -1))
        .expect("#xpath");
    m.define_method("at_xpath", method!(node_at_xpath, -1))
        .expect("#at_xpath");
}
