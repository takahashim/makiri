//! The engine's driver (mkr_xpath.c): the context's lifetime, its namespace and
//! variable registries, the accessors every layer reads it through, and the two
//! evaluate entries that drive an instance.
//!
//! The glue holds the context only as a pointer and goes through the accessors,
//! so this file owns the struct outright.

/* Every function here takes pointers its caller already holds. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::own::OwnedText;
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
     * cache through mkr_str_cache_index_put, and the document-order index is
     * built and cleared by C helpers. */
    str_cache: StrCache,
    order_index: OrderIndex,

    /* The borrowed document-level element index plus its hooks, injected by the
     * glue before evaluation - the engine never sees the index's concrete type.
     * A null index or hook disables the fast path and the engine walks. */
    element_index: *mut c_void,
    tag_lookup: TagIndexLookup,
    tag_has_foreign: TagIndexForeign,

    /* The XML element-name index: the owning document plus a lazy getter and a
     * string-keyed lookup, injected by the XML glue. HTML uses the tag-id index
     * above instead. */
    name_index_owner: *mut c_void,
    name_index_get: NameIndexGet,
    name_index_lookup: NameIndexLookup,

    /* Namespace matching for UNPREFIXED name tests. 0 (default) is strict and
     * HTML5-faithful: an unprefixed name resolves in the HTML namespace, so
     * foreign SVG/MathML needs a prefix. 1 is lax: match by local name. */
    unprefixed_lax: c_int,

    /* Which instance walks this context's nodes: 0 = HTML, 1 = XML. Only the
     * two node-dereferencing entries dispatch on it; everything else per
     * evaluate is representation-neutral. */
    engine_kind: c_int,

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
pub use crate::xpath::ast_ops::mkr_node_clear_memos;
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::mkr_eval_ast_html;
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::mkr_try_first_match_html;

/* Without `lexbor` there is no HTML instance, and no HTML context can be built
 * either - `engine_kind` is always XML - so the HTML arm of the two dispatches
 * below is unreachable. These stand in for it and FAIL CLOSED rather than being
 * a second implementation: reaching them would be a bug, not a slow path. */
#[cfg(not(feature = "lexbor"))]
unsafe fn mkr_eval_ast_html(
    _ctx: *mut Context,
    _ast: *const Node,
    _out: *mut Val,
    _err: ErrSink,
) -> c_int {
    XP_ERR_INTERNAL
}

#[cfg(not(feature = "lexbor"))]
unsafe fn mkr_try_first_match_html(
    _ctx: *mut Context,
    _ast: *const Node,
    _out_node: *mut *mut c_void,
    _err: ErrSink,
) -> c_int {
    0
}
pub use crate::xpath::ffi_xml::mkr_eval_ast_xml;
pub use crate::xpath::ffi_xml::mkr_try_first_match_xml;
pub use crate::xpath::runtime_abi::mkr_doc_order_index_init;
pub use crate::xpath::runtime_abi::mkr_str_cache_clear;
pub use crate::xpath::runtime_abi::mkr_str_cache_init;
pub use crate::xpath::runtime_abi::mkr_str_cache_truncate;

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

pub unsafe fn mkr_xpath_context_new(doc: *mut c_void, node: *mut c_void) -> *mut Context {
    // Null on failure: `mkr_xpath_context_new` already documents null as its
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
        element_index: ptr::null_mut(),
        tag_lookup: None,
        tag_has_foreign: None,
        name_index_owner: ptr::null_mut(),
        name_index_get: None,
        name_index_lookup: None,
        unprefixed_lax: 0,
        engine_kind: 0,
        evaluating: 0,
    }) else {
        return ptr::null_mut();
    };
    super::limits::mkr_xpath_limits_init_defaults(&mut ctx.limits);
    mkr_str_cache_init(&mut ctx.str_cache);
    mkr_doc_order_index_init(&mut ctx.order_index);
    Box::into_raw(ctx)
}

pub unsafe fn mkr_xpath_context_free(ctx: *mut Context) {
    if ctx.is_null() {
        return;
    }
    let mut ctx = Box::from_raw(ctx);
    mkr_str_cache_clear(&mut ctx.str_cache);
    mkr_doc_order_index_clear(&mut ctx.order_index);
    /* The Vecs and the box go with the drop. */
}

/// Owner of a context from [`mkr_xpath_context_new`]: dropping it frees the
/// context, so no early return or `?` can leak one.
pub struct OwnedContext(ptr::NonNull<Context>);

impl OwnedContext {
    /// A fresh context, or None when it could not be allocated.
    ///
    /// # Safety
    /// As [`mkr_xpath_context_new`]: `doc` and `node` must outlive the context.
    pub unsafe fn new(doc: *mut c_void, node: *mut c_void) -> Option<Self> {
        ptr::NonNull::new(mkr_xpath_context_new(doc, node)).map(OwnedContext)
    }

    /// The context, for the engine calls that take it raw. Valid while `self` is.
    pub fn as_ptr(&self) -> *mut Context {
        self.0.as_ptr()
    }
}

impl Drop for OwnedContext {
    fn drop(&mut self) {
        // SAFETY: the pointer came from mkr_xpath_context_new, and only this
        // owner frees it.
        unsafe { mkr_xpath_context_free(self.0.as_ptr()) }
    }
}

/* ---------- registries ---------- */

pub unsafe fn mkr_xpath_register_ns(
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

pub unsafe fn mkr_xpath_register_variable_string(
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
pub unsafe fn mkr_ctx_lookup_ns<'a>(ctx: *mut Context, prefix: &[u8]) -> Option<&'a [u8]> {
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
pub unsafe fn mkr_ctx_lookup_variable_text<'a>(
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

getter!(mkr_ctx_document, *mut c_void, doc, ptr::null_mut());
getter!(mkr_ctx_node, *mut c_void, node, ptr::null_mut());
getter!(
    mkr_ctx_element_index,
    *mut c_void,
    element_index,
    ptr::null_mut()
);
getter!(mkr_ctx_tag_lookup, TagIndexLookup, tag_lookup, None);
getter!(
    mkr_ctx_tag_has_foreign,
    TagIndexForeign,
    tag_has_foreign,
    None
);
getter!(
    mkr_ctx_name_index_owner,
    *mut c_void,
    name_index_owner,
    ptr::null_mut()
);
getter!(mkr_ctx_name_index_get, NameIndexGet, name_index_get, None);
getter!(
    mkr_ctx_name_index_lookup,
    NameIndexLookup,
    name_index_lookup,
    None
);
getter!(mkr_ctx_func_resolver, FuncResolver, func_resolver, None);
getter!(
    mkr_xpath_get_user_data,
    *mut c_void,
    user_data,
    ptr::null_mut()
);
getter!(mkr_ctx_unprefixed_lax, c_int, unprefixed_lax, 0);

pub unsafe fn mkr_ctx_limits(ctx: *mut Context) -> *mut Limits {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).limits
    }
}

pub unsafe fn mkr_ctx_str_cache(ctx: *mut Context) -> *mut StrCache {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).str_cache
    }
}

pub unsafe fn mkr_ctx_order_index(ctx: *mut Context) -> *mut OrderIndex {
    if ctx.is_null() {
        ptr::null_mut()
    } else {
        &raw mut (*ctx).order_index
    }
}

pub unsafe fn mkr_ctx_set_node(ctx: *mut Context, node: *mut c_void) {
    if !ctx.is_null() {
        (*ctx).node = node;
    }
}

pub unsafe fn mkr_ctx_set_unprefixed_lax(ctx: *mut Context, lax: c_int) {
    if !ctx.is_null() {
        (*ctx).unprefixed_lax = c_int::from(lax != 0);
    }
}

pub unsafe fn mkr_xpath_set_engine_kind(ctx: *mut Context, kind: c_int) {
    if !ctx.is_null() {
        (*ctx).engine_kind = c_int::from(kind != 0);
    }
}

pub unsafe fn mkr_xpath_context_set_user_data(ctx: *mut Context, user_data: *mut c_void) {
    if !ctx.is_null() {
        (*ctx).user_data = user_data;
    }
}

pub unsafe fn mkr_xpath_set_func_resolver(ctx: *mut Context, resolver: FuncResolver) {
    if !ctx.is_null() {
        (*ctx).func_resolver = resolver;
    }
}

pub unsafe fn mkr_xpath_context_set_element_index(
    ctx: *mut Context,
    index: *mut c_void,
    lookup: TagIndexLookup,
    has_foreign: TagIndexForeign,
) {
    if !ctx.is_null() {
        (*ctx).element_index = index;
        (*ctx).tag_lookup = lookup;
        (*ctx).tag_has_foreign = has_foreign;
    }
}

pub unsafe fn mkr_xpath_context_set_name_index(
    ctx: *mut Context,
    owner: *mut c_void,
    get: NameIndexGet,
    lookup: NameIndexLookup,
) {
    if !ctx.is_null() {
        (*ctx).name_index_owner = owner;
        (*ctx).name_index_get = get;
        (*ctx).name_index_lookup = lookup;
    }
}

/// True while an evaluate() is in progress on this context, nested ones
/// included. The glue uses it to refuse register_namespace / register_variable /
/// node= re-entered from a handler mid-walk: those mutate the live registration
/// tables or the context node the suspended evaluator still borrows.
pub unsafe fn mkr_ctx_is_evaluating(ctx: *mut Context) -> c_int {
    c_int::from(!ctx.is_null() && (*ctx).evaluating > 0)
}

/* ---------- evaluate ---------- */

/// Move an internal value into the public one. The node-set field has a
/// different name on each side; ownership of the items array and the string
/// transfers.
unsafe fn to_public(v: &Val, out: *mut XPathValue) {
    (*out).type_ = v.type_tag();
    match v.get() {
        ValRef::NodeSet(ns) => {
            (*out).u.nodeset.nodes = ns.items;
            (*out).u.nodeset.count = ns.count;
        }
        ValRef::String(t) => (*out).u.string = t,
        ValRef::Number(d) => (*out).u.number = d,
        ValRef::Boolean(b) => (*out).u.boolean = c_int::from(b),
    }
}

pub(crate) unsafe fn eval_compiled(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    if ctx.is_null() || ast.is_null() || out_value.is_null() {
        if !out_error.is_null() {
            crate::err_setf!(
                ErrSink::from_raw(out_error),
                XP_ERR_INTERNAL,
                "mkr_xpath_eval_compiled: bad arguments"
            );
        }
        return -1;
    }
    let mut err = Error {
        status: XP_OK,
        message: ptr::null_mut(),
    };

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

    let mut v = Val::EMPTY;
    let rc = if (*ctx).engine_kind != 0 {
        mkr_eval_ast_xml(handle(ctx), ast, &mut v, ErrSink::new(&mut err))
    } else {
        mkr_eval_ast_html(handle(ctx), ast, &mut v, ErrSink::new(&mut err))
    };
    mkr_str_cache_truncate(&raw mut (*ctx).str_cache, snapshot);
    if !order_was_built && (*ctx).order_index.built != 0 {
        mkr_doc_order_index_clear(&raw mut (*ctx).order_index);
    }
    /* Memoized values are valid only within one evaluate scope, so clear them
     * whether it succeeded or not. */
    mkr_node_clear_memos(ast);
    (*ctx).evaluating -= 1;

    if rc != 0 {
        if out_error.is_null() {
            mkr_xpath_error_clear(&mut err);
        } else {
            *out_error = err;
        }
        return -1;
    }
    to_public(&v, out_value);
    0
}

pub(crate) unsafe fn eval_compiled_first(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    if ctx.is_null() || ast.is_null() || out_value.is_null() {
        if !out_error.is_null() {
            crate::err_setf!(
                ErrSink::from_raw(out_error),
                XP_ERR_INTERNAL,
                "mkr_xpath_eval_compiled_first: bad arguments"
            );
        }
        return -1;
    }
    /* Reset the per-evaluate counters before the fast path: its descendant walk
     * charges every visited node, so it is bounded fail-closed exactly like the
     * full evaluator (which resets these itself). The not-recognised fallback
     * resets them again; the walk only runs for recognised shapes, so nothing
     * double-counts. */
    (*ctx).limits.eval_ops = 0;
    (*ctx).limits.recursion_depth = 0;

    let mut node: *mut c_void = ptr::null_mut();
    let mut err = Error {
        status: XP_OK,
        message: ptr::null_mut(),
    };
    let matched = if (*ctx).engine_kind != 0 {
        mkr_try_first_match_xml(handle(ctx), ast, &mut node, ErrSink::new(&mut err))
    } else {
        mkr_try_first_match_html(handle(ctx), ast, &mut node, ErrSink::new(&mut err))
    };
    if matched < 0 {
        /* Op budget exceeded while walking: fail closed rather than falling back
         * to the full evaluator, which would hit the same wall. */
        if out_error.is_null() {
            mkr_xpath_error_clear(&mut err);
        } else {
            *out_error = err;
        }
        return -1;
    }
    if matched != 0 {
        /* A recognised first-match shape: a 0-or-1-node node-set, without
         * building or sorting the full descendant set. */
        let mut set = NodeSet::EMPTY;
        if !node.is_null()
            && mkr_nodeset_push(
                &mut set,
                node,
                ptr::null_mut(),
                ErrSink::from_raw(out_error),
            )
            .is_err()
        {
            mkr_nodeset_clear(&mut set);
            return -1;
        }
        to_public(&Val::nodeset(set), out_value);
        return 0;
    }
    eval_compiled(ctx, ast, out_value, out_error)
}
