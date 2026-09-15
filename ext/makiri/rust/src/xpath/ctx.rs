//! The engine's driver (mkr_xpath.c): the context's lifetime, its namespace and
//! variable registries, the accessors every layer reads it through, and the two
//! evaluate entries that drive an instance.
//!
//! The glue holds the context only as a pointer and goes through the accessors,
//! so this file owns the struct outright.

/* Every function here takes pointers its caller already holds. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::own::{OwnedText, OwnedVal, Set};
use crate::falloc::Reserve;
use core::ffi::{c_int, c_void};
use core::ptr;

/// Per-context registration caps. These bound an abusive Ruby loop that calls
/// register_namespace / register_variable without limit; far above any real use.
const MAX_NAMESPACES: usize = 65536;
const MAX_VARIABLES: usize = 65536;

struct NsEntry {
    prefix: OwnedText,
    uri: OwnedText,
}

struct VarEntry {
    /// `ptr` null for the unprefixed (only supported) form.
    prefix: OwnedText,
    name: OwnedText,
    value: OwnedText,
}

/// Which representation a context walks, and the index its `//name` fast path
/// reads. Fixed when the context is made, so the instance that dereferences a
/// node handle always matches the document the handle came from.
#[derive(Clone, Copy)]
pub enum Backend {
    /// A Lexbor document. `index` is the parsed document's element index,
    /// borrowed - the document outlives the context - or null to walk instead.
    #[cfg(feature = "lexbor")]
    Html { index: *const c_void },
    /// A Makiri XML arena. `name_index` enables the lazily built element-name
    /// index that hangs off the document itself.
    Xml { name_index: bool },
}

/// `struct mkr_xpath_context_s`, the real thing.
///
/// Its layout is not ABI: no client names the type - the glue holds a
/// `mkr_xpath_context_t *` and calls accessors, and the engine instances only
/// pass it back - so it can be a plain Rust struct with `Vec` registries rather
/// than the hand-grown arrays the C kept. `abi::Context` is the opaque handle
/// everyone else sees, and `handle()` is the one place the two meet.
pub struct Context {
    doc: *mut c_void,
    node: *mut c_void,

    ns: Vec<NsEntry>,
    vars: Vec<VarEntry>,

    limits: Limits,

    /* The custom function resolver, set by the Ruby handler bridge for the
     * duration of one evaluate() and cleared after. */
    user_data: *mut c_void,
    func_resolver: FuncResolver,

    /* Per-evaluate caches. Both stay C layouts: the value body fills the string
     * cache through str_cache_index_put, and the document-order index is
     * built and cleared by C helpers. */
    str_cache: StrCache,
    order_index: OrderIndex,

    /* Namespace matching for UNPREFIXED name tests. 0 (default) is strict and
     * HTML5-faithful: an unprefixed name resolves in the HTML namespace, so
     * foreign SVG/MathML needs a prefix. 1 is lax: match by local name. */
    unprefixed_lax: c_int,

    /* Which instance walks this context's nodes, and its index. Only the two
     * node-dereferencing entries dispatch on it; everything else per evaluate is
     * representation-neutral. */
    backend: Backend,

    /* Re-entrancy depth, >0 while an evaluate() runs on this context. A custom
     * function handler runs arbitrary Ruby mid-walk and could re-enter to mutate
     * this same context; such a mutation can free a registration string the
     * suspended evaluator still borrows, or swap the context node mid-walk. The
     * glue refuses those while this holds. A nested evaluate just stacks. */
    evaluating: c_int,
}

/// The context as its clients see it: a pointer they carry and hand back.
#[inline]
fn handle(ctx: *mut Context) -> *mut Context {
    ctx
}

/* The two node-dereferencing entries, one pair per instance. They are declared
 * rather than reached through the generic engine because which language provides
 * each is a build-time choice: either C instance can still be in the build while
 * the other is Rust. The signatures are node-pointer-only, hence ABI-identical
 * across the two. */
pub use crate::xpath::ast_ops::node_clear_memos;
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::eval_ast_html;
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::try_first_match_html;

pub use crate::xpath::ffi_xml::eval_ast_xml;
pub use crate::xpath::ffi_xml::try_first_match_xml;
pub use crate::xpath::runtime_abi::doc_order_index_init;
pub use crate::xpath::runtime_abi::str_cache_clear;
pub use crate::xpath::runtime_abi::str_cache_init;
pub use crate::xpath::runtime_abi::str_cache_truncate;

/* ---------- text slots ---------- */

fn empty_text() -> OwnedText {
    OwnedText::new()
}

fn text_eq(a: &OwnedText, b: &[u8]) -> bool {
    a.as_slice() == b
}

/// Copy `val` into a fresh owned text, or None on OOM.
unsafe fn copy_text(val: VerifiedText) -> Option<OwnedText> {
    crate::xpath_abi::TextSlot::try_copy(val.into(), ErrSink::silent(), None)
        .ok()
        .map(OwnedText::from_slot)
}

/// Replace a slot's owned text with a fresh copy: copy FIRST, then clear the
/// old, so an OOM leaves the slot intact.
unsafe fn set_slot(slot: &mut OwnedText, val: VerifiedText) -> c_int {
    match copy_text(val) {
        Some(nv) => {
            *slot = nv;
            0
        }
        None => -1,
    }
}

/* ---------- lifetime ---------- */

pub unsafe fn xpath_context_new(
    doc: *mut c_void,
    node: *mut c_void,
    backend: Backend,
) -> *mut Context {
    // Null on failure: `xpath_context_new` already documents null as its
    // OOM answer (the C version returned it from mkr_callocarray), and every
    // caller checks. Aborting here would take the host process down for a
    // failure the API can already express.
    let Ok(mut ctx) = crate::falloc::try_box(Context {
        doc,
        node,
        ns: Vec::new(),
        vars: Vec::new(),
        limits: core::mem::zeroed(),
        user_data: ptr::null_mut(),
        func_resolver: None,
        str_cache: core::mem::zeroed(),
        order_index: core::mem::zeroed(),
        unprefixed_lax: 0,
        backend,
        evaluating: 0,
    }) else {
        return ptr::null_mut();
    };
    super::limits::xpath_limits_init_defaults(&mut ctx.limits);
    str_cache_init(&mut ctx.str_cache);
    doc_order_index_init(&mut ctx.order_index);
    Box::into_raw(ctx)
}

pub unsafe fn xpath_context_free(ctx: *mut Context) {
    if ctx.is_null() {
        return;
    }
    let mut ctx = Box::from_raw(ctx);
    str_cache_clear(&mut ctx.str_cache);
    doc_order_index_clear(&mut ctx.order_index);
    /* The Vecs and the box go with the drop. */
}

/// Owner of a context from [`xpath_context_new`]: dropping it frees the
/// context, so no early return or `?` can leak one.
pub struct OwnedContext(ptr::NonNull<Context>);

impl OwnedContext {
    /// A fresh context, or None when it could not be allocated.
    ///
    /// # Safety
    /// As [`xpath_context_new`]: `doc` and `node` must outlive the context.
    pub unsafe fn new(doc: *mut c_void, node: *mut c_void, backend: Backend) -> Option<Self> {
        ptr::NonNull::new(xpath_context_new(doc, node, backend)).map(OwnedContext)
    }

    /// The context, for the engine calls that take it raw. Valid while `self` is.
    pub fn as_ptr(&self) -> *mut Context {
        self.0.as_ptr()
    }
}

impl Drop for OwnedContext {
    fn drop(&mut self) {
        // SAFETY: the pointer came from xpath_context_new, and only this
        // owner frees it.
        unsafe { xpath_context_free(self.0.as_ptr()) }
    }
}

/* ---------- registries ---------- */

pub unsafe fn xpath_register_ns(
    ctx: *mut Context,
    prefix: VerifiedText,
    uri: VerifiedText,
) -> c_int {
    if ctx.is_null() || prefix.is_absent() || uri.is_absent() {
        return -1;
    }
    let ctx = &mut *ctx;
    /* Replace when the prefix is already registered. */
    for e in ctx.ns.iter_mut() {
        if text_eq(&e.prefix, prefix.as_bytes()) {
            return set_slot(&mut e.uri, uri);
        }
    }
    if ctx.ns.len() >= MAX_NAMESPACES || ctx.ns.mkr_reserve(1).is_err() {
        return -1;
    }
    let (p, u) = match (copy_text(prefix), copy_text(uri)) {
        (Some(p), Some(u)) => (p, u),
        _ => return -1,
    };
    ctx.ns.push(NsEntry { prefix: p, uri: u });
    0
}

pub unsafe fn xpath_register_variable_string(
    ctx: *mut Context,
    name: VerifiedText,
    value: VerifiedText,
) -> c_int {
    if ctx.is_null() || name.is_absent() {
        return -1;
    }
    let ctx = &mut *ctx;
    /* Only unprefixed string variables are supported. A null `value` means the
     * variable is set to empty, which the copy maps to "". */
    for e in ctx.vars.iter_mut() {
        if e.prefix.is_absent() && text_eq(&e.name, name.as_bytes()) {
            return set_slot(&mut e.value, value);
        }
    }
    if ctx.vars.len() >= MAX_VARIABLES || ctx.vars.mkr_reserve(1).is_err() {
        return -1;
    }
    let (n, v) = match (copy_text(name), copy_text(value)) {
        (Some(n), Some(v)) => (n, v),
        _ => return -1,
    };
    ctx.vars.push(VarEntry {
        prefix: empty_text(),
        name: n,
        value: v,
    });
    0
}

/// The URI registered for `prefix`, borrowed from the registry.
///
/// # Safety
/// `ctx` must be null or live. The bytes belong to its namespace registry, which
/// the glue refuses to change during an evaluate: valid for the call, not past it.
pub unsafe fn ctx_lookup_ns<'a>(ctx: *mut Context, prefix: &[u8]) -> Option<&'a [u8]> {
    if ctx.is_null() {
        return None;
    }
    (*ctx)
        .ns
        .iter()
        .find(|e| text_eq(&e.prefix, prefix))
        .map(|e| e.uri.as_slice())
}

/// The string bound to `$prefix:name` (`prefix` is `None` when unprefixed),
/// borrowed from the registry.
///
/// # Safety
/// `ctx` must be null or live. The bytes are valid until the variable is
/// registered again, which the glue refuses during an evaluate.
pub unsafe fn ctx_lookup_variable_text<'a>(
    ctx: *mut Context,
    prefix: Option<&[u8]>,
    name: &[u8],
) -> Option<&'a [u8]> {
    if ctx.is_null() {
        return None;
    }
    (*ctx)
        .vars
        .iter()
        .find(|e| {
            let prefix_match = match prefix {
                None => e.prefix.is_absent(),
                Some(p) => text_eq(&e.prefix, p),
            };
            prefix_match && text_eq(&e.name, name)
        })
        .map(|e| e.value.as_slice())
}

/* ---------- accessors ---------- */

macro_rules! getter {
    ($name:ident, $ty:ty, $field:ident, $null:expr) => {
        pub unsafe fn $name(ctx: *mut Context) -> $ty {
            if ctx.is_null() {
                $null
            } else {
                (*ctx).$field
            }
        }
    };
}

getter!(ctx_document, *mut c_void, doc, ptr::null_mut());
getter!(ctx_node, *mut c_void, node, ptr::null_mut());
getter!(ctx_func_resolver, FuncResolver, func_resolver, None);
getter!(xpath_get_user_data, *mut c_void, user_data, ptr::null_mut());
getter!(ctx_unprefixed_lax, c_int, unprefixed_lax, 0);

pub unsafe fn ctx_backend(ctx: *mut Context) -> Option<Backend> {
    if ctx.is_null() {
        None
    } else {
        Some((*ctx).backend)
    }
}

pub unsafe fn ctx_limits(ctx: *mut Context) -> *mut Limits {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).limits
    }
}

pub unsafe fn ctx_str_cache(ctx: *mut Context) -> *mut StrCache {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).str_cache
    }
}

pub unsafe fn ctx_order_index(ctx: *mut Context) -> *mut OrderIndex {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).order_index
    }
}

pub unsafe fn ctx_set_context_node(ctx: *mut Context, node: *mut c_void) {
    if !ctx.is_null() {
        (*ctx).node = node;
    }
}

pub unsafe fn ctx_set_unprefixed_lax(ctx: *mut Context, lax: c_int) {
    if !ctx.is_null() {
        (*ctx).unprefixed_lax = c_int::from(lax != 0);
    }
}

pub unsafe fn xpath_context_set_user_data(ctx: *mut Context, user_data: *mut c_void) {
    if !ctx.is_null() {
        (*ctx).user_data = user_data;
    }
}

pub unsafe fn xpath_set_func_resolver(ctx: *mut Context, resolver: FuncResolver) {
    if !ctx.is_null() {
        (*ctx).func_resolver = resolver;
    }
}

/// True while an evaluate() is in progress on this context, nested ones
/// included. The glue uses it to refuse register_namespace / register_variable /
/// node= re-entered from a handler mid-walk: those mutate the live registration
/// tables or the context node the suspended evaluator still borrows.
pub unsafe fn ctx_is_evaluating(ctx: *mut Context) -> c_int {
    c_int::from(!ctx.is_null() && (*ctx).evaluating > 0)
}

/* ---------- evaluate ---------- */

/* The error is returned by value on purpose: it holds its message inline so a
 * failure never allocates, and these run once per query, not per node. */

/// The result of an evaluate, owned: dropping it frees the node-set's array or
/// the string.
pub enum XPathValue {
    NodeSet(Set),
    String(OwnedText),
    Number(f64),
    Boolean(bool),
}

impl XPathValue {
    fn from_owned(mut v: OwnedVal) -> XPathValue {
        let val = v.take();
        match val.get() {
            ValRef::NodeSet(ns) => XPathValue::NodeSet(Set::adopt(*ns)),
            ValRef::String(t) => XPathValue::String(OwnedText::from_slot(t)),
            ValRef::Number(d) => XPathValue::Number(d),
            ValRef::Boolean(b) => XPathValue::Boolean(b),
        }
    }
}

/// Evaluate `ast` against the context, with the context node as the focus.
///
/// # Safety
/// `ctx` and `ast` must be live, `ast` parsed for this context's host; the
/// caller holds the GVL.
#[allow(clippy::result_large_err)]
pub unsafe fn evaluate(ctx: *mut Context, ast: *mut Node) -> Result<XPathValue, Error> {
    let mut err = Error::new();
    if ctx.is_null() || ast.is_null() {
        crate::err_setf!(
            ErrSink::new(&mut err),
            XP_ERR_INTERNAL,
            "evaluate: bad arguments"
        );
        return Err(err);
    }

    /* Mark the context as evaluating for the duration of the walk, so a handler
     * that re-enters cannot mutate it out from under the evaluator. Nested
     * evaluates just stack the depth. */
    (*ctx).evaluating += 1;

    /* Per-eval counters reset. ast_nodes is NOT reset: the AST is already built
     * and its budget was checked at parse time. */
    (*ctx).limits.eval_ops = 0;
    (*ctx).limits.recursion_depth = 0;

    /* String-value cache snapshot. A nested eval - a handler calling back into
     * XPath on the same context - sees the outer entries, but anything it adds
     * is discarded on return, so outer borrowed pointers stay valid. */
    let snapshot = (*ctx).str_cache.count;
    /* Document-order index: the outermost evaluate owns the lifecycle. If the
     * outer had not built it, an inner build is cleared at this exit; if the
     * outer HAD built it, leave it so the outer's sorts still see it. */
    let order_was_built = (*ctx).order_index.built != 0;

    let result = match (*ctx).backend {
        Backend::Xml { .. } => eval_ast_xml(handle(ctx), ast, ErrSink::new(&mut err)),
        #[cfg(feature = "lexbor")]
        Backend::Html { .. } => eval_ast_html(handle(ctx), ast, ErrSink::new(&mut err)),
    };
    str_cache_truncate(&raw mut (*ctx).str_cache, snapshot);
    if !order_was_built && (*ctx).order_index.built != 0 {
        doc_order_index_clear(&raw mut (*ctx).order_index);
    }
    /* Memoized values are valid only within one evaluate scope, so clear them
     * whether it succeeded or not. */
    node_clear_memos(ast);
    (*ctx).evaluating -= 1;

    match result {
        Ok(v) => Ok(XPathValue::from_owned(v)),
        Err(_) => Err(err),
    }
}

/// [`evaluate`] through the `at_xpath` first-match fast path when the shape
/// allows it, and the full evaluator otherwise.
///
/// # Safety
/// As [`evaluate`].
#[allow(clippy::result_large_err)]
pub unsafe fn evaluate_first(ctx: *mut Context, ast: *mut Node) -> Result<XPathValue, Error> {
    let mut err = Error::new();
    if ctx.is_null() || ast.is_null() {
        crate::err_setf!(
            ErrSink::new(&mut err),
            XP_ERR_INTERNAL,
            "evaluate_first: bad arguments"
        );
        return Err(err);
    }
    /* Reset the per-evaluate counters before the fast path: its descendant walk
     * charges every visited node, so it is bounded fail-closed exactly like the
     * full evaluator (which resets these itself). The not-recognised fallback
     * resets them again; the walk only runs for recognised shapes, so nothing
     * double-counts. */
    (*ctx).limits.eval_ops = 0;
    (*ctx).limits.recursion_depth = 0;

    let matched = match (*ctx).backend {
        Backend::Xml { .. } => try_first_match_xml(handle(ctx), ast, ErrSink::new(&mut err)),
        #[cfg(feature = "lexbor")]
        Backend::Html { .. } => try_first_match_html(handle(ctx), ast, ErrSink::new(&mut err)),
    };
    match matched {
        /* Op budget exceeded while walking: fail closed rather than falling back
         * to the full evaluator, which would hit the same wall. */
        Err(_) => Err(err),
        /* A recognised first-match shape: a 0-or-1-node node-set, without
         * building or sorting the full descendant set. */
        Ok(Some(node)) => {
            let mut set = Set::new();
            if !node.is_null()
                && nodeset_push(set.as_mut(), node, ptr::null_mut(), ErrSink::new(&mut err))
                    .is_err()
            {
                return Err(err);
            }
            Ok(XPathValue::NodeSet(set))
        }
        Ok(None) => evaluate(ctx, ast),
    }
}
