//! The engine's driver (mkr_xpath.c): the context's lifetime, its namespace and
//! variable registries, the accessors every layer reads it through, and the two
//! evaluate entries that drive an instance.
//!
//! The glue holds the context only as a pointer and goes through the accessors,
//! so this file owns the struct outright.

/* Every function here takes pointers its caller already holds. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use crate::falloc::Reserve;
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use core::ptr;

/// One call the evaluator routes to the custom-function resolver.
pub struct ResolverCall<'a> {
    /// The focus: the context node as the engine's handle, and its position.
    pub node: *mut c_void,
    pub pos: usize,
    pub size: usize,
    /// The namespace URI of the call's prefix, when it had one.
    pub ns_uri: Option<&'a [u8]>,
    /// The function's local name.
    pub local: &'a [u8],
    pub args: &'a [Val],
}

/// What answers the function calls an evaluate has no built-in for: the glue's
/// bridge to a Ruby handler, passed to one evaluate.
///
/// `resolve` gets `data` back, the evaluation's budget, and the call.
/// `Ok(Some(value))` answers it; `Ok(None)` means there is no such function,
/// which the evaluator reports; `Err` is the function's own failure, already
/// written to the budget.
#[derive(Clone, Copy)]
pub struct Handler {
    pub resolve: unsafe fn(
        data: *mut c_void,
        budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<Val>, Reported>,
    pub data: *mut c_void,
}

/// Per-context registration caps. These bound an abusive Ruby loop that calls
/// register_namespace / register_variable without limit; far above any real use.
const MAX_NAMESPACES: usize = 65536;
const MAX_VARIABLES: usize = 65536;

struct NsEntry {
    prefix: Text,
    uri: Text,
}

struct VarEntry {
    /// None for the unprefixed (only supported) form.
    prefix: Option<Text>,
    name: Text,
    value: Text,
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
    /// A Makiri XML arena. Its element-name index hangs off the document itself
    /// and is built on first use.
    Xml,
}

/// `struct mkr_xpath_context_s`, the real thing.
///
/// Its layout is not ABI: no client names the type - the glue holds a
/// `mkr_xpath_context_t *` and calls accessors, and the engine instances only
/// pass it back - so it can be a plain Rust struct with `Vec` registries rather
/// than the hand-grown arrays the C kept. What one evaluate changes lives in its
/// own `eval::Evaluation`, so an evaluate holds this only shared.
pub struct Context {
    doc: *mut c_void,
    node: *mut c_void,

    ns: Vec<NsEntry>,
    vars: Vec<VarEntry>,

    /* The caps every run under this context starts from. Each evaluate and
     * parse charges a `Budget` of its own made from these. */
    limits: Limits,

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
     * glue refuses those while this holds. A nested evaluate just stacks.
     *
     * A `Cell`, because it is the one field that changes while an evaluate
     * holds the context shared. */
    evaluating: Cell<c_int>,
}

impl Context {
    /// The document handle, as the backend stores it.
    pub fn document(&self) -> *mut c_void {
        self.doc
    }

    /// The context node handle.
    pub fn context_node(&self) -> *mut c_void {
        self.node
    }

    /// The caps a run under this context starts from.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub fn lax(&self) -> bool {
        self.unprefixed_lax != 0
    }

    /// The URI registered for `prefix`, borrowed from the registry.
    pub fn lookup_ns(&self, prefix: &[u8]) -> Option<&[u8]> {
        self.ns
            .iter()
            .find(|e| text_eq(&e.prefix, prefix))
            .map(|e| e.uri.as_slice())
    }

    /// The string bound to `$prefix:name` (`prefix` is `None` when
    /// unprefixed), borrowed from the registry.
    pub fn variable_text(&self, prefix: Option<&[u8]>, name: &[u8]) -> Option<&[u8]> {
        self.vars
            .iter()
            .find(|e| {
                let prefix_match = match prefix {
                    None => e.prefix.is_none(),
                    Some(p) => e.prefix.as_ref().is_some_and(|t| t.as_slice() == p),
                };
                prefix_match && text_eq(&e.name, name)
            })
            .map(|e| e.value.as_slice())
    }
}

/* The two node-dereferencing entries, one pair per instance. They are declared
 * rather than reached through the generic engine because which language provides
 * each is a build-time choice: either C instance can still be in the build while
 * the other is Rust. The signatures are node-pointer-only, hence ABI-identical
 * across the two. */
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::eval_ast_html;
#[cfg(feature = "lexbor")]
pub use crate::xpath::ffi_html::try_first_match_html;

pub use crate::xpath::ffi_xml::eval_ast_xml;
pub use crate::xpath::ffi_xml::try_first_match_xml;

/* ---------- text slots ---------- */

fn text_eq(a: &Text, b: &[u8]) -> bool {
    a.as_slice() == b
}

/// Copy `val` into a fresh owned text, or None on OOM.
unsafe fn copy_text(val: VerifiedText) -> Option<Text> {
    Text::try_copy(val.as_bytes())
}

/// Replace a slot's text with a fresh copy: copy FIRST, then drop the old, so
/// an OOM leaves the slot intact.
unsafe fn set_slot(slot: &mut Text, val: VerifiedText) -> c_int {
    match copy_text(val) {
        Some(text) => {
            *slot = text;
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
    // OOM answer (the C version returned it from callocarray), and every
    // caller checks. Aborting here would take the host process down for a
    // failure the API can already express.
    let Ok(ctx) = crate::falloc::try_box(Context {
        doc,
        node,
        ns: Vec::new(),
        vars: Vec::new(),
        limits: Limits::DEFAULT,
        unprefixed_lax: 0,
        backend,
        evaluating: Cell::new(0),
    }) else {
        return ptr::null_mut();
    };
    Box::into_raw(ctx)
}

pub unsafe fn xpath_context_free(ctx: *mut Context) {
    if ctx.is_null() {
        return;
    }
    /* The Vecs and the box go with the drop. */
    drop(Box::from_raw(ctx));
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
        if e.prefix.is_none() && text_eq(&e.name, name.as_bytes()) {
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
        prefix: None,
        name: n,
        value: v,
    });
    0
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

/// True while an evaluate() is in progress on this context, nested ones
/// included. The glue uses it to refuse register_namespace / register_variable /
/// node= re-entered from a handler mid-walk: those mutate the live registration
/// tables or the context node the suspended evaluator still borrows.
pub unsafe fn ctx_is_evaluating(ctx: *mut Context) -> c_int {
    c_int::from(!ctx.is_null() && (*ctx).evaluating.get() > 0)
}

/* ---------- evaluate ---------- */

/* The error is returned by value on purpose: it holds its message inline so a
 * failure never allocates, and these run once per query, not per node. */

/// The result of an evaluate, owned: dropping it frees the node-set's array or
/// the string.
pub type XPathValue = Val;

/// Evaluate `ast` against the context, with the context node as the focus;
/// `handler` answers the function calls there is no built-in for.
///
/// Each call runs on an evaluation of its own - its budget, its caches - and
/// only reads the context, so a handler that evaluates again on this same
/// context cannot disturb the walk it was called from.
///
/// # Safety
/// `ctx` must be live and `ast` parsed for this context's host; `handler`'s
/// data must stay valid for the call; the caller holds the GVL.
#[allow(clippy::result_large_err)]
pub unsafe fn evaluate(
    ctx: *mut Context,
    ast: &Ast,
    handler: Option<Handler>,
) -> Result<XPathValue, Error> {
    let Some(cx) = ctx.as_ref() else {
        let mut err = Error::new();
        crate::err_setf!(
            ErrSink::new(&mut err),
            XP_ERR_INTERNAL,
            "evaluate: bad arguments"
        );
        return Err(err);
    };

    /* Mark the context as evaluating for the duration of the walk, so a handler
     * that re-enters cannot mutate it out from under the evaluator. Nested
     * evaluates just stack the depth. */
    cx.evaluating.set(cx.evaluating.get() + 1);
    let result = match cx.backend {
        Backend::Xml => eval_ast_xml(cx, ast, handler),
        #[cfg(feature = "lexbor")]
        Backend::Html { .. } => eval_ast_html(cx, ast, handler),
    };
    cx.evaluating.set(cx.evaluating.get() - 1);
    result
}

/// [`evaluate`] through the `at_xpath` first-match fast path when the shape
/// allows it, and the full evaluator otherwise.
///
/// # Safety
/// As [`evaluate`].
#[allow(clippy::result_large_err)]
pub unsafe fn evaluate_first(
    ctx: *mut Context,
    ast: &Ast,
    handler: Option<Handler>,
) -> Result<XPathValue, Error> {
    let Some(cx) = ctx.as_ref() else {
        let mut err = Error::new();
        crate::err_setf!(
            ErrSink::new(&mut err),
            XP_ERR_INTERNAL,
            "evaluate_first: bad arguments"
        );
        return Err(err);
    };
    /* The fast path charges every visited node to a budget of its own, so it is
     * bounded fail-closed exactly like the full evaluator; it only runs for
     * recognised shapes, which call no functions, so it needs no handler. */
    let matched = match cx.backend {
        Backend::Xml => try_first_match_xml(cx, ast)?,
        #[cfg(feature = "lexbor")]
        Backend::Html { .. } => try_first_match_html(cx, ast)?,
    };
    match matched {
        /* A recognised first-match shape: a 0-or-1-node node-set, without
         * building or sorting the full descendant set. */
        Some(node) => {
            let mut set = NodeSet::new();
            let mut budget = Budget::with_limits(cx.limits);
            if set.push_token(node, &mut budget).is_err() {
                return Err(budget.take_error());
            }
            Ok(XPathValue::NodeSet(set))
        }
        None => evaluate(ctx, ast, handler),
    }
}
