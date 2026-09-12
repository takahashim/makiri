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
//! # Two functions here are not only ours
//!
//! [`mkr_xpath_raise`] and [`mkr_xpath_value_to_ruby`] are called by the XML
//! query glue as well, so they keep their C names and are defined here. Dropping
//! the C file without providing them would leave an unresolved symbol that macOS
//! turns into a NULL jump at runtime rather than a link error.

#![allow(clippy::missing_safety_doc)]

use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_void};
use std::collections::HashMap;

use magnus::gc::Marker;
use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::value::{Opaque, ReprValue};
use magnus::{method, prelude::*, DataTypeFunctions, Error, RClass, Ruby, TypedData, Value};
use rb_sys::VALUE;

use crate::xpath_abi::{
    Error as XPathError, Limits, Node as Ast, NodeSet, OwnedText, Val, VerifiedText, XPathValue,
    XP_ERR_LIMIT, XP_ERR_RUNTIME, XP_ERR_SYNTAX,
};

use super::abi::{
    error_class, is_kind_of, mkr_cNode, mkr_cNodeSet, mkr_cXmlDocument, mkr_doc_parsed, mkr_html_node_unwrap,
    mkr_mHtmlNodeMethods, mkr_node_document, mkr_node_raw, mkr_node_set_new, mkr_node_set_push,
    mkr_parsed_xml_doc,
};

/// An `XPathContext` is typically reused to run the same handful of expressions
/// many times, so each is parsed once and the AST re-evaluated (the evaluator
/// resets the per-eval counters and memo slots, so a cached AST is safely
/// reusable). Bounded, so a context fed unbounded distinct expressions cannot
/// grow without limit.
const AST_CACHE_MAX: usize = 1024;

/// Upper bound on handler arguments, matching the engine's default
/// `max_function_args`. The resolver refuses any call above it, so the fixed
/// argv array below cannot overflow however the limit is tuned - and the stack
/// use stays independent of the runtime argument count.
const HANDLER_MAX_ARGS: usize = 64;

const MKR_XPATH_TYPE_NODESET: u32 = 0;
const MKR_XPATH_TYPE_STRING: u32 = 1;
const MKR_XPATH_TYPE_NUMBER: u32 = 2;
const MKR_XPATH_TYPE_BOOLEAN: u32 = 3;

/// `mkr_doc_kind_t`.
const MKR_DOC_XML: c_int = 1;

/// `mkr_xpath_context_s`, opaque.
#[repr(C)]
struct Ctx {
    _private: [u8; 0],
}

extern "C" {
    static mkr_cXPathContext: VALUE;
    static mkr_eXPathSyntaxError: VALUE;
    static mkr_eXPathLimitExceeded: VALUE;

    fn mkr_xpath_context_new(doc: *mut c_void, node: *mut c_void) -> *mut Ctx;
    fn mkr_xpath_context_free(ctx: *mut Ctx);
    fn mkr_xpath_set_engine_kind(ctx: *mut Ctx, kind: c_int);
    fn mkr_ctx_set_unprefixed_lax(ctx: *mut Ctx, lax: c_int);
    fn mkr_ctx_set_node(ctx: *mut Ctx, node: *mut c_void);
    fn mkr_ctx_is_evaluating(ctx: *mut Ctx) -> c_int;
    fn mkr_ctx_limits(ctx: *mut Ctx) -> *mut Limits;
    fn mkr_xpath_context_set_element_index(
        ctx: *mut Ctx,
        index: *mut c_void,
        tag: *const c_void,
        has_foreign: *const c_void,
    );
    fn mkr_xpath_context_set_user_data(ctx: *mut Ctx, data: *mut c_void);
    fn mkr_xpath_set_func_resolver(ctx: *mut Ctx, resolver: *const c_void);

    fn mkr_parsed_kind(p: *const c_void) -> c_int;
    fn mkr_parsed_dom_index_build(p: *mut c_void) -> c_int;
    fn mkr_parsed_element_index(p: *mut c_void) -> *mut c_void;
    static mkr_element_index_tag: c_void;
    static mkr_element_index_has_foreign: c_void;
    fn mkr_html_doc_unwrap(rb_doc: VALUE) -> *mut c_void;
    fn mkr_xml_node_unwrap(v: VALUE) -> *mut c_void;

    fn mkr_parse(expr: VerifiedText, limits: *mut Limits, err: *mut XPathError) -> *mut Ast;
    fn mkr_node_free(ast: *mut Ast);
    fn mkr_xpath_eval_compiled(
        ctx: *mut Ctx,
        ast: *mut Ast,
        out: *mut XPathValue,
        err: *mut XPathError,
    ) -> c_int;
    fn mkr_xpath_eval_compiled_first(
        ctx: *mut Ctx,
        ast: *mut Ast,
        out: *mut XPathValue,
        err: *mut XPathError,
    ) -> c_int;
    fn mkr_xpath_value_clear(v: *mut XPathValue);
    fn mkr_xpath_error_clear(e: *mut XPathError);
    fn mkr_err_set(err: *mut XPathError, status: c_int, msg: *const c_char);

    fn mkr_xpath_register_ns(ctx: *mut Ctx, prefix: VerifiedText, uri: VerifiedText) -> c_int;
    fn mkr_xpath_register_variable_string(
        ctx: *mut Ctx,
        name: VerifiedText,
        value: VerifiedText,
    ) -> c_int;

    fn mkr_nodeset_init(ns: *mut NodeSet);
    fn mkr_nodeset_push(
        ns: *mut NodeSet,
        node: *mut c_void,
        limits: *mut Limits,
        err: *mut XPathError,
    ) -> c_int;
    fn mkr_nodeset_clear(ns: *mut NodeSet);
    fn mkr_val_set_borrowed_text_copy(
        v: *mut Val,
        text: VerifiedText,
        err: *mut XPathError,
        what: *const c_char,
    ) -> c_int;

    fn mkr_ruby_verified_text(input: VALUE, what: *const c_char) -> RubyText;
    fn mkr_ruby_try_verified_text(
        sv: VALUE,
        max_bytes: usize,
        out: *mut RubyText,
    ) -> *const c_char;
    fn mkr_ruby_exception_message(exc: VALUE, buf: *mut c_char, len: usize);
}

/// `mkr_ruby_borrowed_text_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct RubyText {
    value: VALUE,
    ptr: *const c_char,
    len: usize,
}

impl From<RubyText> for VerifiedText {
    fn from(t: RubyText) -> Self {
        VerifiedText { ptr: t.ptr, len: t.len }
    }
}

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/// Turn an engine error into a Ruby exception and raise. Never returns.
///
/// Exported: the XML query glue raises through this too, so an engine failure
/// maps to the same exception whichever entry point produced it.
#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_raise(err: *mut XPathError) -> ! {
    let class = match (*err).status {
        XP_ERR_SYNTAX => mkr_eXPathSyntaxError,
        XP_ERR_LIMIT => mkr_eXPathLimitExceeded,
        _ => error_class().as_raw(),
    };
    /* Copy the message out before clearing the native error. */
    let msg = if (*err).message.is_null() {
        rb_sys::rb_utf8_str_new_cstr(c"XPath evaluation failed".as_ptr())
    } else {
        rb_sys::rb_utf8_str_new_cstr((*err).message)
    };
    mkr_xpath_error_clear(err);
    rb_sys::rb_exc_raise(rb_sys::rb_exc_new_str(class, msg))
}

/// Convert a just-produced engine value into a Ruby object, then release the
/// heap the engine handed us. `document` is the keepalive for a node-set.
///
/// Exported for the same reason as [`mkr_xpath_raise`].
#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_value_to_ruby(v: *mut XPathValue, document: VALUE) -> VALUE {
    let result = match (*v).type_ {
        MKR_XPATH_TYPE_NODESET => {
            let set = mkr_node_set_new(document);
            let ns = (*v).u.nodeset;
            for i in 0..ns.count {
                mkr_node_set_push(set, *ns.nodes.add(i));
            }
            set
        }
        MKR_XPATH_TYPE_STRING => owned_text_to_str((*v).u.string),
        MKR_XPATH_TYPE_NUMBER => rb_sys::rb_float_new((*v).u.number),
        MKR_XPATH_TYPE_BOOLEAN => {
            if (*v).u.boolean != 0 {
                rb_sys::Qtrue as VALUE
            } else {
                rb_sys::Qfalse as VALUE
            }
        }
        _ => rb_sys::Qnil as VALUE,
    };
    mkr_xpath_value_clear(v);
    result
}

/// An engine string as a UTF-8 Ruby String. A NULL pointer is `""`.
unsafe fn owned_text_to_str(t: OwnedText) -> VALUE {
    let p = if t.ptr.is_null() { c"".as_ptr() } else { t.ptr };
    let n = if t.ptr.is_null() { 0 } else { t.len };
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
struct AstCache(HashMap<Box<[u8]>, *mut Ast>);

impl Drop for AstCache {
    fn drop(&mut self) {
        for (_, ast) in self.0.drain() {
            // SAFETY: every value was produced by mkr_parse and is owned here.
            unsafe { mkr_node_free(ast) };
        }
    }
}

struct Inner {
    ctx: *mut Ctx,
    cache: AstCache,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            // SAFETY: paired with mkr_xpath_context_new; the cache drops first.
            unsafe { mkr_xpath_context_free(self.ctx) };
        }
    }
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
        Ok(self.borrow()?.ctx)
    }
}

/* ------------------------------------------------------------------ */
/* context construction                                               */
/* ------------------------------------------------------------------ */

/// The three symbols the keyword check compares against, interned once.
/// `to_symbol` is a lookup, and this runs on the per-call path.
unsafe fn kw_symbols() -> (VALUE, VALUE, VALUE) {
    static mut SYMS: (VALUE, VALUE, VALUE) = (0, 0, 0);
    if SYMS.0 == 0 {
        let sym = |s: &core::ffi::CStr| rb_sys::rb_id2sym(rb_sys::rb_intern(s.as_ptr()));
        SYMS = (
            sym(c"namespace_matching"),
            sym(c"strict"),
            sym(c"lax"),
        );
    }
    SYMS
}

/// Resolve the `namespace_matching:` keyword to the unprefixed-lax flag.
///
/// `:strict` (the default) resolves an unprefixed name test in the HTML
/// namespace, which is what browsers do; `:lax` makes it namespace-agnostic.
fn ns_matching_lax(ruby: &Ruby, opts: magnus::RHash) -> Result<c_int, Error> {
    if opts.is_empty() {
        return Ok(0);
    }
    let (key, strict, lax) = unsafe { kw_symbols() };
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
        format!("namespace_matching: must be :strict or :lax, got {}", v.inspect()),
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
unsafe fn context_for(rb_node: Value, document: Value) -> Result<*mut Ctx, Error> {
    let parsed = mkr_doc_parsed(document.as_raw());

    if mkr_parsed_kind(parsed) == MKR_DOC_XML {
        let xdoc = mkr_parsed_xml_doc(parsed);
        let docn = if xdoc.is_null() {
            core::ptr::null_mut()
        } else {
            (*(xdoc as *mut crate::xml::abi::Doc)).doc_node as *mut c_void
        };
        let cnode = if is_kind_of(rb_node, mkr_cXmlDocument) {
            docn
        } else {
            mkr_xml_node_unwrap(rb_node.as_raw())
        };
        let xctx = mkr_xpath_context_new(docn, cnode);
        if xctx.is_null() {
            return Err(Error::new(error_class(), "failed to allocate XPath context"));
        }
        mkr_xpath_set_engine_kind(xctx, 1);
        return Ok(xctx);
    }

    let node = mkr_html_node_unwrap(rb_node.as_raw());
    let doc = mkr_html_doc_unwrap(document.as_raw());
    if mkr_parsed_dom_index_build(parsed) != 0 {
        return Err(Error::new(
            error_class(),
            "failed to build attribute index for XPath",
        ));
    }
    let ctx = mkr_xpath_context_new(doc, node as *mut c_void);
    if ctx.is_null() {
        return Err(Error::new(error_class(), "failed to allocate XPath context"));
    }
    /* Borrowed: the index lives on the parsed document, which outlives this
     * context. The engine calls back through the hooks and never sees its type. */
    mkr_xpath_context_set_element_index(
        ctx,
        mkr_parsed_element_index(parsed),
        &mkr_element_index_tag as *const c_void,
        &mkr_element_index_has_foreign as *const c_void,
    );
    Ok(ctx)
}

/// `XPathContext.new(node, namespace_matching: :strict)`.
fn ctx_s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let rb_node = a.required.0;
    let lax = ns_matching_lax(ruby, a.keywords)?;

    if !unsafe { is_kind_of(rb_node, mkr_cNode) } {
        return Err(Error::new(ruby.exception_type_error(), "expected a Makiri::Node"));
    }
    let document = unsafe { Value::from_raw(mkr_node_document(rb_node.as_raw())) };
    let ctx = unsafe { context_for(rb_node, document)? };
    unsafe { mkr_ctx_set_unprefixed_lax(ctx, lax) };

    Ok(ruby
        .wrap(XPathCtx {
            document: document.into(),
            node: Cell::new(rb_node.into()),
            inner: RefCell::new(Inner { ctx, cache: AstCache(HashMap::new()) }),
        })
        .as_value())
}

/// `#node=` - rebind the context node, so one context can evaluate relative
/// expressions against several nodes. Namespace and variable registrations are
/// preserved. The node must be in the same document.
fn ctx_set_node(ruby: &Ruby, rb_self: &XPathCtx, rb_node: Value) -> Result<Value, Error> {
    if !unsafe { is_kind_of(rb_node, mkr_cNode) } {
        return Err(Error::new(ruby.exception_type_error(), "expected a Makiri::Node"));
    }
    let ctx = rb_self.ctx()?;
    unsafe {
        if mkr_ctx_is_evaluating(ctx) != 0 {
            return Err(Error::new(
                error_class(),
                "cannot change the context node while evaluating (re-entrant mutation from a handler)",
            ));
        }
        if mkr_node_document(rb_node.as_raw()) != ruby.get_inner(rb_self.document).as_raw() {
            return Err(Error::new(
                error_class(),
                "context node must belong to the same document",
            ));
        }
        rb_self.node.set(rb_node.into()); /* keepalive; marked above */
        /* Same-document is verified, so rb_node is the context's representation
         * and the engine - monomorphized per kind - takes the raw pointer. */
        mkr_ctx_set_node(ctx, mkr_node_raw(rb_node.as_raw()));
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
    match v.type_ {
        MKR_XPATH_TYPE_NODESET => {
            let set = mkr_node_set_new(b.document);
            let ns = v.u.nodeset;
            for i in 0..ns.count {
                mkr_node_set_push(set, *ns.items.add(i));
            }
            set
        }
        MKR_XPATH_TYPE_STRING => owned_text_to_str(v.u.string),
        MKR_XPATH_TYPE_NUMBER => rb_sys::rb_float_new(v.u.number),
        MKR_XPATH_TYPE_BOOLEAN => {
            if v.u.boolean != 0 {
                rb_sys::Qtrue as VALUE
            } else {
                rb_sys::Qfalse as VALUE
            }
        }
        _ => rb_sys::Qnil as VALUE,
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
    out: *mut Val,
    err: &mut ErrBuf,
) -> bool {
    if mkr_node_document(rb_node) != document {
        err.set("handler returned a node from a different document");
        return false;
    }
    let n = mkr_node_raw(rb_node);
    let mut ierr: XPathError = core::mem::zeroed();
    if mkr_nodeset_push(&mut (*out).u.nodeset, n, mkr_ctx_limits(ctx), &mut ierr) != 0 {
        mkr_xpath_error_clear(&mut ierr);
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
        ErrBuf { buf: [0; 200], len: 0 }
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
unsafe fn ruby_to_out(ctx: *mut Ctx, document: VALUE, r: VALUE, out: *mut Val, err: &mut ErrBuf) -> bool {
    let rv = Value::from_raw(r);
    if r == rb_sys::Qtrue as VALUE || r == rb_sys::Qfalse as VALUE {
        (*out).type_ = MKR_XPATH_TYPE_BOOLEAN;
        (*out).u.boolean = c_int::from(r == rb_sys::Qtrue as VALUE);
        return true;
    }
    let ruby = Ruby::get_unchecked();
    if is_numeric(&ruby, rv) {
        let Ok(f) = f64::try_convert(rv) else {
            err.set("handler returned a number that could not be read");
            return false;
        };
        (*out).type_ = MKR_XPATH_TYPE_NUMBER;
        (*out).u.number = f;
        return true;
    }
    let is_node = is_kind_of(rv, mkr_cNode);
    if is_node || is_kind_of(rv, mkr_cNodeSet) {
        (*out).type_ = MKR_XPATH_TYPE_NODESET;
        mkr_nodeset_init(&mut (*out).u.nodeset);
        if is_node {
            if !push_result_node(ctx, document, r, out, err) {
                mkr_nodeset_clear(&mut (*out).u.nodeset);
                return false;
            }
        } else {
            let Ok(n) = rv.funcall::<_, _, i64>("length", ()) else {
                mkr_nodeset_clear(&mut (*out).u.nodeset);
                err.set("handler result could not be read");
                return false;
            };
            for i in 0..n {
                let Ok(node) = rv.funcall::<_, _, Value>("[]", (i,)) else { continue };
                if !is_kind_of(node, mkr_cNode) {
                    continue;
                }
                if !push_result_node(ctx, document, node.as_raw(), out, err) {
                    mkr_nodeset_clear(&mut (*out).u.nodeset);
                    return false;
                }
            }
        }
        return true;
    }

    /* nil and everything else: coerce to a string (nil -> ""). */
    if rv.is_nil() {
        if mkr_val_set_borrowed_text_copy(
            out,
            VerifiedText { ptr: c"".as_ptr(), len: 0 },
            core::ptr::null_mut(),
            core::ptr::null(),
        ) != 0
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
    let mut vv: RubyText = core::mem::zeroed();
    let bad = mkr_ruby_try_verified_text(sv.as_raw(), (*mkr_ctx_limits(ctx)).max_string_bytes, &mut vv);
    if !bad.is_null() {
        let reason = core::ffi::CStr::from_ptr(bad).to_string_lossy();
        err.set_fmt(format_args!("handler returned an invalid string: {reason}"));
        return false;
    }
    let rc = mkr_val_set_borrowed_text_copy(
        out,
        vv.into(),
        core::ptr::null_mut(),
        core::ptr::null(),
    );
    core::hint::black_box(sv);
    if rc != 0 || (*out).u.string.ptr.is_null() {
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

/// The engine's resolver hook. 0 = handled, -1 = errored, +1 = not found.
unsafe extern "C" fn handler_resolver(
    user_data: *mut c_void,
    ctx: *mut Ctx,
    _self_node: *mut c_void,
    _self_pos: usize,
    _self_size: usize,
    _ns_uri: *const c_char,
    local_name: *const c_char,
    args: *mut c_void,
    nargs: usize,
    out: *mut c_void,
    err: *mut XPathError,
) -> c_int {
    if user_data.is_null() || local_name.is_null() {
        return 1;
    }
    let bridge = &*(user_data as *const Bridge);
    if bridge.handler == rb_sys::Qnil as VALUE {
        return 1;
    }

    /* The method name: XPath uses '-', Ruby uses '_'. */
    let mut name = [0u8; 128];
    let mut n = 0usize;
    while n + 1 < name.len() {
        let b = *local_name.add(n) as u8;
        if b == 0 {
            break;
        }
        name[n] = if b == b'-' { b'_' } else { b };
        n += 1;
    }
    if *local_name.add(n) as u8 != 0 {
        return 1; /* too long to map to a Ruby method name */
    }
    name[n] = 0;

    let method = rb_sys::rb_intern(name.as_ptr() as *const c_char);
    if rb_sys::rb_respond_to(bridge.handler, method) == 0 {
        return 1; /* let the engine raise "unknown function" */
    }

    if nargs > HANDLER_MAX_ARGS {
        let mut b = ErrBuf::new();
        b.set_fmt(format_args!(
            "handler function '{}' called with too many arguments ({} > {})",
            core::str::from_utf8_unchecked(&name[..n]),
            nargs,
            HANDLER_MAX_ARGS
        ));
        mkr_err_set(err, XP_ERR_RUNTIME, b.as_ptr());
        return -1;
    }

    let mut call = HandlerCall {
        bridge,
        ctx,
        method,
        args: args as *const Val,
        nargs,
        out: out as *mut Val,
        ok: true,
        err: ErrBuf::new(),
        argv: [rb_sys::Qnil as VALUE; HANDLER_MAX_ARGS],
    };

    let mut state: c_int = 0;
    rb_sys::rb_protect(
        Some(handler_call_body),
        &mut call as *mut HandlerCall as VALUE,
        &mut state,
    );
    if state != 0 {
        let exc = rb_sys::rb_errinfo();
        rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
        let mut msg = [0i8; 200];
        mkr_ruby_exception_message(exc, msg.as_mut_ptr(), msg.len());
        let mut b = ErrBuf::new();
        b.set_fmt(format_args!(
            "handler raised: {}",
            core::ffi::CStr::from_ptr(msg.as_ptr()).to_string_lossy()
        ));
        mkr_err_set(err, XP_ERR_RUNTIME, b.as_ptr());
        return -1;
    }
    if !call.ok {
        mkr_err_set(err, XP_ERR_RUNTIME, call.err.as_ptr());
        return -1;
    }
    0
}

/* ------------------------------------------------------------------ */
/* evaluate                                                           */
/* ------------------------------------------------------------------ */

/// The compiled AST for `expr`, parsing and caching it on first use.
///
/// Returns `(ast, owned)`; `owned` means the cache did not take it and the
/// caller must free it.
unsafe fn cached_ast(
    d: &mut Inner,
    expr: RubyText,
    err: *mut XPathError,
) -> Option<(*mut Ast, bool)> {
    let key = if expr.ptr.is_null() || expr.len == 0 {
        &[][..]
    } else {
        core::slice::from_raw_parts(expr.ptr as *const u8, expr.len)
    };
    if let Some(&ast) = d.cache.0.get(key) {
        return Some((ast, false));
    }

    let limits = mkr_ctx_limits(d.ctx);
    (*limits).ast_nodes = 0;
    let ast = mkr_parse(expr.into(), limits, err);
    if ast.is_null() {
        return None;
    }
    if d.cache.0.len() >= AST_CACHE_MAX || d.cache.0.try_reserve(1).is_err() {
        return Some((ast, true));
    }
    d.cache.0.insert(key.to_vec().into_boxed_slice(), ast);
    Some((ast, false))
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
            mkr_xpath_context_set_user_data(ctx, bridge as *mut c_void);
            mkr_xpath_set_func_resolver(ctx, handler_resolver as *const c_void);
        }
        InstalledHandler { ctx, installed }
    }
}

impl Drop for InstalledHandler {
    fn drop(&mut self) {
        if self.installed {
            // SAFETY: undoes exactly what new() did.
            unsafe {
                mkr_xpath_set_func_resolver(self.ctx, core::ptr::null());
                mkr_xpath_context_set_user_data(self.ctx, core::ptr::null_mut());
            }
        }
    }
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
        let mut d = rb_self.borrow()?;
        let ev = mkr_ruby_verified_text(expr.as_raw(), c"XPath expression".as_ptr());
        let mut error: XPathError = core::mem::zeroed();
        let parsed = cached_ast(&mut d, ev, &mut error);
        core::hint::black_box(expr);
        let ctx = d.ctx;
        match parsed {
            Some((ast, owned)) => (ctx, ast, owned),
            None => {
                /* rb_raise longjmps, which would leave the RefCell marked in
                 * use forever; release it first. */
                drop(d);
                mkr_xpath_raise(&mut error);
            }
        }
    };

    unsafe {
        let bridge = Bridge { handler: handler.as_raw(), document: document.as_raw() };
        let installed = InstalledHandler::new(ctx, &bridge, handler.as_raw());
        let mut value: XPathValue = core::mem::zeroed();
        let mut error: XPathError = core::mem::zeroed();
        let rc = mkr_xpath_eval_compiled(ctx, ast, &mut value, &mut error);
        drop(installed);
        if owned {
            mkr_node_free(ast);
        }
        if rc != 0 {
            mkr_xpath_raise(&mut error);
        }
        Ok(Value::from_raw(mkr_xpath_value_to_ruby(&mut value, document.as_raw())))
    }
}

fn ctx_register_ns(rb_self: &XPathCtx, prefix: Value, uri: Value) -> Result<Value, Error> {
    let ctx = rb_self.ctx()?;
    unsafe {
        if mkr_ctx_is_evaluating(ctx) != 0 {
            return Err(Error::new(
                error_class(),
                "cannot register a namespace while evaluating (re-entrant mutation from a handler)",
            ));
        }
        let pv = mkr_ruby_verified_text(prefix.as_raw(), c"namespace prefix".as_ptr());
        let uv = mkr_ruby_verified_text(uri.as_raw(), c"namespace URI".as_ptr());
        let rc = mkr_xpath_register_ns(ctx, pv.into(), uv.into()); /* copies both */
        core::hint::black_box((prefix, uri));
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
        if mkr_ctx_is_evaluating(ctx) != 0 {
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
        let nv = mkr_ruby_verified_text(name.as_raw(), c"variable name".as_ptr());
        let mut vv: RubyText = core::mem::zeroed();
        let bad =
            mkr_ruby_try_verified_text(sv.as_raw(), (*mkr_ctx_limits(ctx)).max_string_bytes, &mut vv);
        if !bad.is_null() {
            let reason = core::ffi::CStr::from_ptr(bad).to_string_lossy().into_owned();
            return Err(Error::new(
                error_class(),
                format!("invalid variable value: {reason}"),
            ));
        }
        let rc = mkr_xpath_register_variable_string(ctx, nv.into(), vv.into()); /* copies both */
        core::hint::black_box((name, sv));
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
        let document = Value::from_raw(mkr_node_document(rb_self.as_raw()));
        let ev = mkr_ruby_verified_text(expr.as_raw(), c"XPath expression".as_ptr());

        let ctx = context_for(rb_self, document)?;
        mkr_ctx_set_unprefixed_lax(ctx, lax);

        let mut error: XPathError = core::mem::zeroed();
        let limits = mkr_ctx_limits(ctx);
        (*limits).ast_nodes = 0;
        let ast = mkr_parse(ev.into(), limits, &mut error);
        core::hint::black_box(expr); /* keep the expression's bytes alive */
        if ast.is_null() {
            mkr_xpath_context_free(ctx);
            mkr_xpath_raise(&mut error);
        }

        let bridge = Bridge { handler: handler.as_raw(), document: document.as_raw() };
        let installed = InstalledHandler::new(ctx, &bridge, handler.as_raw());
        let mut value: XPathValue = core::mem::zeroed();
        let rc = if first_only {
            mkr_xpath_eval_compiled_first(ctx, ast, &mut value, &mut error)
        } else {
            mkr_xpath_eval_compiled(ctx, ast, &mut value, &mut error)
        };
        drop(installed);
        mkr_node_free(ast);
        if rc != 0 {
            mkr_xpath_context_free(ctx);
            mkr_xpath_raise(&mut error);
        }
        /* Free the context BEFORE converting: the value owns its own data and
         * never references the context, so a raise inside the conversion (the
         * node-set cap, OOM) cannot leak it. */
        mkr_xpath_context_free(ctx);
        Ok(Value::from_raw(mkr_xpath_value_to_ruby(&mut value, document.as_raw())))
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
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), magnus::RHash, ()>(args)?;
    let lax = ns_matching_lax(ruby, a.keywords)?;
    Ok((a.required.0, a.optional.0.unwrap_or(ruby.qnil().as_value()), lax))
}

fn node_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let (expr, handler, lax) = scan_query_args(ruby, args)?;
    node_xpath_run(rb_self, expr, handler, lax, false)
}

/// The first matching node for a node-set result, or the scalar otherwise.
fn node_at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let (expr, handler, lax) = scan_query_args(ruby, args)?;
    let result = node_xpath_run(rb_self, expr, handler, lax, true)?;
    if unsafe { is_kind_of(result, mkr_cNodeSet) } {
        return result.funcall("first", ());
    }
    Ok(result)
}

/// # Safety
/// From `Init_makiri`.
#[no_mangle]
pub unsafe extern "C" fn mkr_init_xpath() {
    let klass = RClass::from_value(Value::from_raw(mkr_cXPathContext))
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
    klass.define_method("node=", method!(ctx_set_node, 1)).expect("#node=");

    let m = magnus::RModule::from_value(Value::from_raw(mkr_mHtmlNodeMethods))
        .expect("Makiri::HTML::NodeMethods");
    m.define_method("xpath", method!(node_xpath, -1)).expect("#xpath");
    m.define_method("at_xpath", method!(node_at_xpath, -1)).expect("#at_xpath");
}
