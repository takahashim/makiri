//! The XPath 1.0 evaluator (mkr_xpath_eval_body.h): axis walks, node tests,
//! predicates, the operator semantics, and the two index fast paths.
//!
//! Generic over `Dom`, so one body compiles per representation - what the C
//! achieves by `#include`-ing this file twice behind different macros.

/* The failure detail lives in the `*mut Error` the C caller owns, exactly as it
 * does in the C, so the Rust error type carries nothing. */
#![allow(clippy::result_unit_err)]

use super::abi::*;
use super::dom::*;
use super::funcs::{self, Focus};
use super::own::{OwnedVal, Set, Text};
use super::value::*;
use crate::err_setf;
use core::ffi::{c_char, c_void};
use core::ptr;

/* mkr_axis_t, repeated here so the evaluator reads like the grammar. */
use super::abi::{
    AXIS_ANCESTOR, AXIS_ANCESTOR_OR_SELF, AXIS_ATTRIBUTE, AXIS_CHILD, AXIS_DESCENDANT,
    AXIS_DESCENDANT_OR_SELF, AXIS_FOLLOWING, AXIS_FOLLOWING_SIBLING, AXIS_NAMESPACE, AXIS_PARENT,
    AXIS_PRECEDING, AXIS_PRECEDING_SIBLING, AXIS_SELF,
};

#[inline]
fn node_void<D: Dom>(n: D::Node) -> *mut c_void {
    D::to_void(n)
}

#[inline]
unsafe fn void_node<D: Dom>(p: *mut c_void) -> D::Node {
    D::from_void(p)
}

/* ---------- node tests ---------- */

/// What a name test needs from the context, resolved ONCE per step rather than
/// per visited node.
///
/// The C reads all three through the context on every node. It gets away with
/// two of them for free: `MKR_NODE_NS_URI`'s `doc` argument does not appear in
/// the XML expansion, so the macro never evaluates it, and the lax flag is only
/// consulted for a node that has a namespace. Neither shortcut survives a real
/// function call, so they are hoisted here instead - a name-test walk is the
/// hottest loop in the engine.
#[derive(Clone, Copy)]
pub struct Bindings<'a, D: Dom> {
    pub ctx: *mut Context,
    pub doc: D::Doc,
    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub lax: bool,
    /// The name test's prefix, already resolved to a URI.
    pub pre: Option<&'a [u8]>,
}

impl<'a, D: Dom> Bindings<'a, D> {
    /// # Safety
    /// `ctx` must be the evaluating context.
    pub unsafe fn new(ctx: *mut Context, pre: Option<&'a [u8]>) -> Bindings<'a, D> {
        Bindings {
            ctx,
            doc: D::doc_from_void(mkr_ctx_document(ctx)),
            lax: mkr_ctx_unprefixed_lax(ctx) != 0,
            pre,
        }
    }
}

/// The host-specific element / attribute name match. The principal-node-type
/// filter has already passed; this decides name and namespace.
///
/// XML compares the LOCAL name plus the namespace URI: an unprefixed test
/// matches a no-namespace node only (a prefixed or default-namespaced element
/// needs a registered prefix), and a prefixed test matches the URI bound to the
/// prefix. HTML uses the qualified-name model instead: an unprefixed test
/// compares the qualified name (which for HTML is the local name) and, in strict
/// mode, restricts elements to the HTML or null namespace.
///
/// `pre` carries the prefix's already-resolved URI, so a hot multi-node walk
/// resolves it once in `eval_step` rather than per node.
unsafe fn name_test_match<D: Dom>(
    test: *const NodeTest,
    node: D::Node,
    axis: u32,
    b: &Bindings<D>,
) -> bool {
    let want_local = owned_bytes((*test).local);
    if (*test).local.ptr.is_null() {
        return false;
    }
    let is_attr = axis == AXIS_ATTRIBUTE;
    let prefixed = !(*test).prefix.ptr.is_null();

    let got: &[u8] = if D::IS_XML || prefixed {
        if is_attr {
            D::attr_local_name(node)
        } else {
            D::local_name(node)
        }
    } else if is_attr {
        D::attr_qualified_name(node)
    } else {
        D::qualified_name(node)
    };
    if got != want_local {
        return false;
    }

    if prefixed {
        let want_uri = match resolved_prefix(b, test) {
            Some(u) => u,
            None => return false, /* unknown prefix -> non-match; the step driver reports it */
        };
        return want_uri == D::ns_uri(node, b.doc);
    }
    if b.lax {
        return true;
    }
    if D::IS_XML {
        /* strict unprefixed: the node must be in no namespace */
        D::ns_uri(node, b.doc).is_empty()
    } else {
        /* strict: unprefixed ELEMENT tests resolve in the HTML namespace, so a
         * foreign (SVG / MathML) element needs a prefix. Attributes are exempt -
         * an unprefixed attribute test matches by no-namespace local name, and
         * the qualified-name compare above already excluded prefixed foreign
         * attributes. */
        is_attr || !D::is_foreign_ns(node)
    }
}

unsafe fn resolved_prefix<'a, D: Dom>(
    b: &Bindings<'a, D>,
    test: *const NodeTest,
) -> Option<&'a [u8]> {
    match b.pre {
        Some(u) => Some(u),
        None => lookup_ns(b.ctx, owned_bytes((*test).prefix)),
    }
}

unsafe fn lookup_ns<'a>(ctx: *mut Context, prefix: &[u8]) -> Option<&'a [u8]> {
    let mut len = 0usize;
    let p = mkr_ctx_lookup_ns(ctx, prefix.as_ptr() as *const c_char, prefix.len(), &mut len);
    if p.is_null() {
        None
    } else if len == 0 {
        Some(&[])
    } else {
        Some(core::slice::from_raw_parts(p as *const u8, len))
    }
}

unsafe fn node_principal_match<D: Dom>(
    test: *const NodeTest,
    node: D::Node,
    axis: u32,
    b: &Bindings<D>,
) -> bool {
    match (*test).kind {
        NT_NODE => {
            /* §5's data model has only element, attribute, text, namespace, PI,
             * comment and the root. Both representations additionally carry
             * DOCUMENT_TYPE / ENTITY / ENTITY_REFERENCE / NOTATION nodes, which
             * are not in the model, so node() must not match them. A
             * DocumentFragment IS matched: it is the root of a fragment-rooted
             * context, so '.' over a fragment has to see it. */
            !matches!(
                D::node_type(node),
                NTYPE_DOCUMENT_TYPE | NTYPE_ENTITY | NTYPE_ENTITY_REFERENCE | NTYPE_NOTATION
            )
        }
        NT_TEXT => matches!(D::node_type(node), NTYPE_TEXT | NTYPE_CDATA_SECTION),
        NT_COMMENT => D::node_type(node) == NTYPE_COMMENT,
        NT_PI => {
            if D::node_type(node) != NTYPE_PI {
                return false;
            }
            if (*test).pi_target.ptr.is_null() {
                return true;
            }
            D::pi_name(node) == owned_bytes((*test).pi_target)
        }
        NT_WILDCARD => {
            if axis == AXIS_NAMESPACE {
                return false;
            }
            /* the principal node type of the axis */
            if axis == AXIS_ATTRIBUTE {
                if D::node_type(node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(node) != NTYPE_ELEMENT {
                return false;
            }
            /* `*` matches any namespace; `prefix:*` only the one bound to the
             * prefix. An unknown prefix is reported up front by the step driver;
             * here it is a non-match. */
            if (*test).prefix.ptr.is_null() {
                return true;
            }
            match resolved_prefix(b, test) {
                Some(want) => want == D::ns_uri(node, b.doc),
                None => false,
            }
        }
        NT_NAME => {
            if axis == AXIS_ATTRIBUTE {
                if D::node_type(node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(node) != NTYPE_ELEMENT {
                return false;
            }
            name_test_match::<D>(test, node, axis, b)
        }
        _ => false,
    }
}

/* ---------- axis walks ---------- */

/// Pre-order DFS over `context`'s PROPER descendants, calling `visit` on each;
/// stops as soon as `visit` returns true. The shared body of the descendant and
/// descendant-or-self axes - the latter only visits `context` first.
unsafe fn walk_descendants<D: Dom, F: FnMut(D::Node) -> bool>(
    context: D::Node,
    visit: &mut F,
) -> bool {
    let mut n = D::first_child(context);
    while !D::is_null(n) && n != context {
        if visit(n) {
            return true;
        }
        if !D::is_null(D::first_child(n)) {
            n = D::first_child(n);
        } else {
            while n != context && D::is_null(D::next(n)) {
                n = D::parent(n);
            }
            if n == context {
                break;
            }
            n = D::next(n);
        }
    }
    false
}

/// Where a document-order axis walk starts. For an attribute context node that
/// is its owner element.
///
/// §2.2 keeps attribute nodes out of the following / preceding axes, and §5.3
/// puts an element's attributes before its children, so a walk beginning at the
/// attribute itself would have to skip past the rest of the attribute list.
/// Starting at the owner gets there directly, and matches libxml2:
/// `following::node()` from an attribute yields what comes after the owner
/// element's subtree, not the element's own children.
unsafe fn axis_base<D: Dom>(context: D::Node) -> D::Node {
    if D::node_type(context) == NTYPE_ATTRIBUTE {
        let owner = D::parent(context);
        if !D::is_null(owner) {
            return owner;
        }
    }
    context
}

unsafe fn walk_axis<D: Dom, F: FnMut(D::Node) -> bool>(
    axis: u32,
    context: D::Node,
    visit: &mut F,
) -> bool {
    match axis {
        AXIS_SELF => visit(context),
        AXIS_PARENT => {
            let p = D::parent(context);
            !D::is_null(p) && visit(p)
        }
        AXIS_CHILD => {
            let mut c = D::first_child(context);
            while !D::is_null(c) {
                if visit(c) {
                    return true;
                }
                c = D::next(c);
            }
            false
        }
        AXIS_ATTRIBUTE => {
            if D::node_type(context) != NTYPE_ELEMENT {
                return false;
            }
            let mut a = D::first_attr(context);
            while !D::is_null(a) {
                if visit(a) {
                    return true;
                }
                a = D::attr_next(a);
            }
            false
        }
        AXIS_DESCENDANT_OR_SELF => visit(context) || walk_descendants::<D, F>(context, visit),
        AXIS_DESCENDANT => walk_descendants::<D, F>(context, visit),
        AXIS_ANCESTOR => {
            let mut p = D::parent(context);
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(p);
            }
            false
        }
        AXIS_ANCESTOR_OR_SELF => {
            let mut p = context;
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(p);
            }
            false
        }
        /* §2.2: both sibling axes are empty for an attribute context node - an
         * attribute is not a sibling of anything. */
        AXIS_FOLLOWING_SIBLING => {
            if D::node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::next(context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::next(s);
            }
            false
        }
        AXIS_PRECEDING_SIBLING => {
            if D::node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::prev(context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::prev(s);
            }
            false
        }
        AXIS_FOLLOWING => {
            /* Start at the next node in document order after the base's subtree. */
            let mut cur = axis_base::<D>(context);
            while !D::is_null(cur) && D::is_null(D::next(cur)) {
                cur = D::parent(cur);
            }
            if D::is_null(cur) {
                return false;
            }
            cur = D::next(cur);
            while !D::is_null(cur) {
                if visit(cur) {
                    return true;
                }
                if !D::is_null(D::first_child(cur)) {
                    cur = D::first_child(cur);
                } else {
                    while !D::is_null(cur) && D::is_null(D::next(cur)) {
                        cur = D::parent(cur);
                    }
                    if !D::is_null(cur) {
                        cur = D::next(cur);
                    }
                }
            }
            false
        }
        AXIS_PRECEDING => {
            /* Backward in document order, skipping the context's ancestors, so
             * the closest preceding node comes first.
             *
             * Climbing to a parent reaches an ancestor only when we are climbing
             * the chain from the context itself, not when climbing back out of a
             * preceding sibling's subtree - hence the explicit test. It stays
             * anchored on `context`, not the base: for an attribute context node
             * the owner element IS an ancestor (§2.2), so starting the walk there
             * must not emit it. */
            let mut cur = axis_base::<D>(context);
            while !D::is_null(cur) {
                if !D::is_null(D::prev(cur)) {
                    cur = D::prev(cur);
                    while !D::is_null(D::last_child(cur)) {
                        cur = D::last_child(cur);
                    }
                    if visit(cur) {
                        return true;
                    }
                } else {
                    cur = D::parent(cur);
                    if D::is_null(cur) {
                        return false;
                    }
                    let mut is_ancestor = false;
                    let mut p = D::parent(context);
                    while !D::is_null(p) {
                        if p == cur {
                            is_ancestor = true;
                            break;
                        }
                        p = D::parent(p);
                    }
                    if !is_ancestor && visit(cur) {
                        return true;
                    }
                }
            }
            false
        }
        /* The namespace axis is rejected by the step driver before it gets here. */
        _ => false,
    }
}

/* ---------- the [@name] / [@name='lit'] predicate fast path ---------- */

/// The two most common predicate shapes. The generic evaluator services them by
/// building a throwaway node-set per context node plus a string-cache insert;
/// recognising the shape filters with a direct attribute lookup instead. Both
/// are boolean - no position dependence - so this is a pure per-node filter with
/// the same result as the generic path.
struct AttrPred<'a> {
    name: &'a [u8],
    value: Option<&'a [u8]>,
}

/// Shape A: a relative path that is one unprefixed attribute name test with no
/// predicates - `@name`.
unsafe fn match_attr_step<'a>(n: *const Node) -> Option<&'a [u8]> {
    if n.is_null() || (*n).kind != NK_PATH || (*n).u.path.absolute != 0 || (*n).u.path.nsteps != 1 {
        return None;
    }
    let s = &*(*n).u.path.steps;
    if s.axis != AXIS_ATTRIBUTE
        || s.npredicates != 0
        || s.test.kind != NT_NAME
        || !s.test.prefix.ptr.is_null()
        || s.test.local.ptr.is_null()
    {
        return None;
    }
    Some(owned_bytes(s.test.local))
}

/// `[@name]`, or `[@name='lit']` in either operand order.
unsafe fn match_attr_pred<'a>(p: *const Node) -> Option<AttrPred<'a>> {
    if let Some(name) = match_attr_step(p) {
        return Some(AttrPred { name, value: None });
    }
    if p.is_null() || (*p).kind != NK_BINOP || (*p).u.binop.op != OP_EQ {
        return None;
    }
    let (lhs, rhs) = ((*p).u.binop.lhs, (*p).u.binop.rhs);
    let (lit, attr) = if !lhs.is_null() && (*lhs).kind == NK_LITERAL_STR {
        (lhs, rhs)
    } else if !rhs.is_null() && (*rhs).kind == NK_LITERAL_STR {
        (rhs, lhs)
    } else {
        return None;
    };
    let name = match_attr_step(attr)?;
    Some(AttrPred { name, value: Some(owned_bytes((*lit).u.literal)) })
}

/// The attribute whose QUALIFIED name is exactly `name`, case-sensitively.
///
/// This scans rather than using the host's attribute lookup, because Lexbor's is
/// HTML case-INsensitive - which would make `[@Id]` match `id`, diverging from
/// XPath 1.0, from Nokogiri::HTML5, and from Makiri's own attribute-axis name
/// test, which compares the qualified name byte for byte. The fast path handles
/// unprefixed names only, matching that comparison.
unsafe fn attr_by_qualified_name<D: Dom>(el: D::Node, name: &[u8]) -> D::Node {
    let mut a = D::first_attr(el);
    while !D::is_null(a) {
        if D::attr_qualified_name(a) == name {
            return a;
        }
        a = D::attr_next(a);
    }
    D::null()
}

/// THE single per-node test for a recognised attribute predicate, shared by the
/// predicate filter and the at_xpath first-match path so the two stay identical
/// by construction rather than by a hand-kept copy.
unsafe fn attr_pred_matches<D: Dom>(ap: &AttrPred, n: D::Node) -> bool {
    if D::node_type(n) != NTYPE_ELEMENT {
        return false;
    }
    let a = attr_by_qualified_name::<D>(n, ap.name);
    if D::is_null(a) {
        return false;
    }
    match ap.value {
        None => true,
        Some(want) => D::attr_value(a) == want,
    }
}

/// A path's step list, empty when there are none.
unsafe fn path_steps<'a>(steps: *mut Step, n: usize) -> &'a [Step] {
    if n == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(steps, n)
    }
}

/// A step's predicate list. Empty when there are none, so the pointer is never
/// read for a count of zero.
unsafe fn step_preds<'a>(step: *const Step) -> &'a [*mut Node] {
    if (*step).npredicates == 0 {
        &[]
    } else {
        core::slice::from_raw_parts((*step).predicates, (*step).npredicates)
    }
}

/* ---------- predicates ---------- */

unsafe fn apply_predicates<D: Dom>(
    ctx: *mut Context,
    preds: &[*mut Node],
    inout: &mut Set,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    for &pred in preds {
        let mut kept = Set::new();

        /* Specialise [@name] / [@name='lit'] - position-independent, so applying
         * it per predicate even amid others matches the generic path. */
        if let Some(ap) = match_attr_pred(pred) {
            for i in 0..inout.count() {
                /* Charge per candidate: this replaces a per-node generic
                 * predicate eval, which would tick through eval_node, so the
                 * shortcut stays under the same budget as the path it skips. */
                if mkr_limit_eval_op(limits, err) != 0 {
                    return false;
                }
                let n = inout.get::<D>(i);
                if attr_pred_matches::<D>(&ap, n) && !kept.push::<D>(n, limits, err) {
                    return false;
                }
            }
            inout.replace(kept.take());
            continue;
        }

        let size = inout.count();
        for i in 0..size {
            let n = inout.get::<D>(i);
            let mut v = OwnedVal::new();
            let pf = Focus::<D> { node: n, pos: i + 1, size };
            if !eval_node::<D>(ctx, pred, &pf, v.as_mut(), err) {
                return false;
            }
            /* A bare number predicate means position() = that number. */
            let keep = if (*v.as_ptr()).type_ == T_NUMBER {
                (*v.as_ptr()).u.number == (i + 1) as f64
            } else {
                val_to_boolean(v.as_ptr())
            };
            if keep && !kept.push::<D>(n, limits, err) {
                return false;
            }
        }
        inout.replace(kept.take());
    }
    true
}

/* ---------- steps ---------- */

/// Can walking `axis` from distinct context nodes yield the same node twice?
///
/// child, attribute and self each anchor a result to one starting node, so
/// distinct contexts give distinct results. Everything else can overlap: two
/// contexts share a parent, or sit in an ancestor-descendant relation.
fn axis_can_alias(a: u32) -> bool {
    !matches!(a, AXIS_CHILD | AXIS_ATTRIBUTE | AXIS_SELF)
}

fn axis_is_implemented(a: u32) -> bool {
    a != AXIS_NAMESPACE && a <= AXIS_ANCESTOR_OR_SELF
}

fn axis_name(a: u32) -> &'static str {
    match a {
        AXIS_ANCESTOR => "ancestor",
        AXIS_ANCESTOR_OR_SELF => "ancestor-or-self",
        AXIS_FOLLOWING => "following",
        AXIS_PRECEDING => "preceding",
        AXIS_FOLLOWING_SIBLING => "following-sibling",
        AXIS_PRECEDING_SIBLING => "preceding-sibling",
        AXIS_NAMESPACE => "namespace",
        _ => "axis",
    }
}

/// A reverse axis in the §2.4 sense: the walker emits in reverse-document order
/// and proximity position() counts outward from the context node. The step
/// driver applies predicates in that axis-natural order (so `[1]` is the
/// closest), then sorts the merged result into document order.
fn is_reverse_axis(a: u32) -> bool {
    matches!(
        a,
        AXIS_ANCESTOR | AXIS_ANCESTOR_OR_SELF | AXIS_PRECEDING | AXIS_PRECEDING_SIBLING
    )
}

/// Is the context exactly the document node? Both index fast paths need that:
/// `descendant::tag` from the document is precisely "every element named tag",
/// which is what the index groups.
unsafe fn context_is_document<D: Dom>(ctx: *mut Context, set: &Set) -> bool {
    set.count() == 1 && node_void::<D>(set.get::<D>(0)) == mkr_ctx_document(ctx)
}

/// `//tag` from the index instead of a tree walk. Returns Ok(true) when it
/// filled `result`, Ok(false) when the shape does not qualify.
unsafe fn try_descendant_index<D: Dom>(
    step: *const Step,
    context_set: &Set,
    result: &mut Set,
    b: &Bindings<D>,
    err: *mut Error,
) -> Result<bool, ()> {
    let test = &raw const (*step).test;
    if (*step).axis != AXIS_DESCENDANT
        || (*test).kind != NT_NAME
        || (*test).local.ptr.is_null()
        || !context_is_document::<D>(b.ctx, context_set)
    {
        return Ok(false);
    }
    let ns_uri = if (*test).prefix.ptr.is_null() { None } else { b.pre };
    if !(*test).prefix.ptr.is_null() && ns_uri.is_none() {
        return Ok(false); /* eval_step pre-resolves, so this should not happen */
    }
    let bucket = match D::name_bucket(b.ctx, owned_bytes((*test).local), ns_uri, b.lax) {
        Some(bk) => bk,
        None => return Ok(false),
    };
    let limits = mkr_ctx_limits(b.ctx);
    for &p in bucket.nodes {
        if mkr_limit_eval_op(limits, err) != 0 {
            return Err(());
        }
        let n = void_node::<D>(p);
        if bucket.recheck && !node_principal_match::<D>(test, n, (*step).axis, b) {
            continue;
        }
        if !result.push::<D>(n, limits, err) {
            return Err(());
        }
    }
    Ok(true)
}

unsafe fn eval_step<D: Dom>(
    ctx: *mut Context,
    step: *const Step,
    context_set: &Set,
    out: &mut Set,
    err: *mut Error,
) -> bool {
    let axis = (*step).axis;
    if !axis_is_implemented(axis) {
        err_setf!(
            err,
            XP_ERR_NOT_IMPLEMENTED,
            "native engine: axis '{}' not implemented yet",
            axis_name(axis)
        );
        return false;
    }
    let test = &raw const (*step).test;
    let limits = mkr_ctx_limits(ctx);

    /* Resolve the namespace prefix once up front (covering `prefix:local` and
     * `prefix:*`): a uniform RUNTIME error rather than a silently empty match,
     * and every per-node match then reuses the URI instead of re-resolving.
     *
     * The borrow lives in the context's registry and cannot be freed mid-
     * evaluate: the only path that frees it is re-registering the same prefix,
     * and the glue refuses register_namespace (and register_variable, node=)
     * while an evaluate is in progress on this context - which is exactly when a
     * predicate handler could re-enter. */
    let pre: Option<&[u8]> = if (*test).prefix.ptr.is_null() {
        None
    } else {
        match lookup_ns(ctx, owned_bytes((*test).prefix)) {
            Some(u) => Some(u),
            None => {
                err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}' in name test",
                    Bytes(owned_bytes((*test).prefix))
                );
                return false;
            }
        }
    };

    let b = Bindings::<D>::new(ctx, pre);

    /* A post-pass (sort to document order, then optional adjacent dedup) is
     * needed when the axis emits in reverse order per context, when it aliases
     * across contexts, or when several contexts produce results that interleave.
     * child is the canonical third case: html's children are head and body, and
     * head's children include title, so naive concatenation gives
     * [head, body, title] where document order is [head, title, body]. Only self
     * and attribute stay in order under concatenation. */
    let need_post_pass = is_reverse_axis(axis)
        || (axis_can_alias(axis) && context_set.count() > 1)
        || (context_set.count() > 1 && axis != AXIS_SELF && axis != AXIS_ATTRIBUTE);

    let mut result = Set::new();

    let preds = step_preds(step);
    if preds.is_empty() {
        match try_descendant_index::<D>(step, context_set, &mut result, &b, err) {
            Err(()) => return false,
            Ok(true) => {}
            Ok(false) => {
                /* No-predicate walk: every context goes straight into the result
                 * buffer regardless of the post-pass, saving the per-context
                 * fragment the predicate path needs. */
                let mut aborted = false;
                for ci in 0..context_set.count() {
                    let mut visit = |n: D::Node| -> bool {
                        /* Charge every visited node. The axis walk is the
                         * dominant work of a step, and a low-selectivity walk
                         * name-tests many nodes while pushing few - so without
                         * this the node-set cap, which bounds only what is
                         * pushed, leaves the walk itself bounded by document
                         * size, defeating max_eval_ops on a descendant walk that
                         * matches nothing. */
                        if mkr_limit_eval_op(limits, err) != 0 {
                            aborted = true;
                            return true;
                        }
                        if node_principal_match::<D>(test, n, axis, &b)
                            && !result.push::<D>(n, limits, err)
                        {
                            aborted = true;
                            return true;
                        }
                        false
                    };
                    walk_axis::<D, _>(axis, context_set.get::<D>(ci), &mut visit);
                    if aborted {
                        return false;
                    }
                }
            }
        }
    } else {
        /* Predicate path: position() and last() are per-context, so each
         * context's fragment has to be materialised before filtering. One
         * fragment buffer is reused across iterations, so its storage grows to
         * the largest single-context cardinality once rather than per iteration. */
        let mut fragment = Set::new();
        for ci in 0..context_set.count() {
            fragment.0.count = 0;
            let mut aborted = false;
            {
                let frag = &mut fragment;
                let mut visit = |n: D::Node| -> bool {
                    if mkr_limit_eval_op(limits, err) != 0 {
                        aborted = true;
                        return true;
                    }
                    if node_principal_match::<D>(test, n, axis, &b)
                        && !frag.push::<D>(n, limits, err)
                    {
                        aborted = true;
                        return true;
                    }
                    false
                };
                walk_axis::<D, _>(axis, context_set.get::<D>(ci), &mut visit);
            }
            if aborted {
                return false;
            }

            /* Predicates apply per context with axis-natural position numbering
             * (§2.4). For a reverse axis the fragment is in reverse-document
             * order, so [1] is the closest to the context - the intended
             * meaning. */
            if !apply_predicates::<D>(ctx, preds, &mut fragment, err) {
                return false;
            }
            for i in 0..fragment.count() {
                if !result.push::<D>(fragment.get::<D>(i), limits, err) {
                    return false;
                }
            }
        }
    }

    if need_post_pass && result.count() > 1 {
        nodeset_unique_sorted::<D>(ctx, result.as_mut());
    }
    out.replace(result.take());
    true
}

/* ---------- the `//name[N]` index fast path ---------- */

/// `//name[N]` - the two leading steps `descendant-or-self::node()` and
/// `child::name[N]`, rooted at the document - selects, for every node, its Nth
/// name-child. That is NOT `(//name)[N]` and not `descendant::name[N]`.
///
/// The index lists matching elements in document order, so a parent's
/// name-children appear among them in child order: one sweep with a
/// pointer-keyed parent -> count map emits exactly those whose running count
/// reaches N, already in document order, with no sort or dedup.
unsafe fn nth_shape<D: Dom>(
    ctx: *mut Context,
    s0: *const Step,
    s1: *const Step,
    seed: &Set,
) -> Option<usize> {
    if (*s0).axis != AXIS_DESCENDANT_OR_SELF
        || (*s0).test.kind != NT_NODE
        || !(*s0).test.prefix.ptr.is_null()
        || (*s0).npredicates != 0
    {
        return None;
    }
    if (*s1).axis != AXIS_CHILD
        || (*s1).test.kind != NT_NAME
        || (*s1).test.local.ptr.is_null()
        || (*s1).npredicates != 1
    {
        return None;
    }
    /* The sole predicate must be a bare positive-integer literal, which is
     * position() == N. `[position()=N]` and `[last()]` are binops or calls and
     * fall back. */
    let pred = step_preds(s1)[0];
    if pred.is_null() || (*pred).kind != NK_LITERAL_NUM {
        return None;
    }
    let dn = (*pred).u.literal_num;
    /* NaN is spelled out rather than left to a negated comparison: `[NaN]`
     * must fall back, and `!(dn >= 1.0)` says so only by accident. */
    if dn.is_nan() || dn < 1.0 || dn != dn.trunc() || dn > usize::MAX as f64 {
        return None;
    }
    if !context_is_document::<D>(ctx, seed) {
        return None;
    }
    Some(dn as usize)
}

unsafe fn try_descendant_index_nth<D: Dom>(
    ctx: *mut Context,
    s0: *const Step,
    s1: *const Step,
    seed: &Set,
    result: &mut Set,
    err: *mut Error,
) -> Result<bool, ()> {
    let need = match nth_shape::<D>(ctx, s0, s1, seed) {
        Some(n) => n,
        None => return Ok(false),
    };
    let test = &raw const (*s1).test;
    let ns_uri: Option<&[u8]> = if (*test).prefix.ptr.is_null() {
        None
    } else {
        match lookup_ns(ctx, owned_bytes((*test).prefix)) {
            Some(u) => Some(u),
            None => {
                err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}' in name test",
                    Bytes(owned_bytes((*test).prefix))
                );
                return Err(());
            }
        }
    };
    let b = Bindings::<D>::new(ctx, ns_uri);
    let bucket = match D::name_bucket(ctx, owned_bytes((*test).local), ns_uri, b.lax) {
        Some(bk) => bk,
        None => return Ok(false),
    };
    if bucket.nodes.is_empty() {
        return Ok(true);
    }

    /* A pointer-keyed count per parent. Sized from the bucket so the open
     * addressing stays under a 2/3 load; an overflow in the sizer falls back to
     * the generic evaluator rather than risking a table that never finds a slot. */
    let want = bucket.nodes.len() + (bucket.nodes.len() >> 1) + 1;
    let cap = want.checked_next_power_of_two().ok_or(())?;
    let mut tab: Vec<(*const c_void, usize)> = Vec::new();
    if tab.try_reserve_exact(cap).is_err() {
        err_setf!(err, XP_ERR_OOM, "out of memory (//name[N])");
        return Err(());
    }
    tab.resize(cap, (ptr::null(), 0));
    let mask = cap - 1;
    let limits = mkr_ctx_limits(ctx);

    for &p in bucket.nodes {
        if mkr_limit_eval_op(limits, err) != 0 {
            return Err(());
        }
        let e = void_node::<D>(p);
        if bucket.recheck && !node_principal_match::<D>(test, e, (*s1).axis, &b) {
            continue;
        }
        let par = node_void::<D>(D::parent(e)) as *const c_void;
        let mut h = (ptr_hash(par) as usize) & mask;
        while !tab[h].0.is_null() && tab[h].0 != par {
            h = (h + 1) & mask;
        }
        tab[h].0 = par;
        tab[h].1 += 1;
        if tab[h].1 == need && !result.push::<D>(e, limits, err) {
            return Err(());
        }
    }
    Ok(true)
}

unsafe fn eval_steps<D: Dom>(
    ctx: *mut Context,
    steps: &[Step],
    seed: &mut Set,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let mut current = Set::adopt(seed.take());
    let mut rest = steps;

    if let [s0, s1, ..] = steps {
        let mut nth = Set::new();
        match try_descendant_index_nth::<D>(ctx, s0, s1, &current, &mut nth, err) {
            Err(()) => return false,
            Ok(true) => {
                current = Set::adopt(nth.take());
                rest = &steps[2..];
            }
            Ok(false) => {}
        }
    }
    for step in rest {
        let mut next = Set::new();
        if !eval_step::<D>(ctx, step, &current, &mut next, err) {
            return false;
        }
        current = Set::adopt(next.take());
    }
    (*out).type_ = T_NODESET;
    (*out).u.nodeset = current.take();
    true
}

/* ---------- comparisons ---------- */

/// §3.4 equality. A node-set on either side means "true iff SOME node satisfies
/// it"; all node string-values go through the per-evaluate cache, so an M-by-N
/// comparison costs O(M+N) string builds.
unsafe fn compare_eq<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    op: u32,
    err: *mut Error,
) -> Option<bool> {
    let limits = mkr_ctx_limits(ctx);
    let want_eq = op == OP_EQ;
    let (lt, rt) = ((*l).type_, (*r).type_);

    if lt == T_NODESET && rt == T_NODESET {
        /* The pair scan itself is M*N even though the string builds are O(M+N),
         * so charge each pair: otherwise an all-pairs node-set equality drives
         * up to ~1e14 comparisons as a handful of ops. */
        let (ls, rs) = (&raw const (*l).u.nodeset, &raw const (*r).u.nodeset);
        for i in 0..(*ls).count {
            let a = cached_node_text::<D>(ctx, nodeset_at::<D>(ls, i), err)?;
            for j in 0..(*rs).count {
                if mkr_limit_eval_op(limits, err) != 0 {
                    return None;
                }
                let b = cached_node_text::<D>(ctx, nodeset_at::<D>(rs, j), err)?;
                if (a == b) == want_eq {
                    return Some(true);
                }
            }
        }
        return Some(false);
    }
    if lt == T_NODESET || rt == T_NODESET {
        let (ns, sc) = if lt == T_NODESET { (l, r) } else { (r, l) };
        let set = &raw const (*ns).u.nodeset;
        match (*sc).type_ {
            T_NUMBER => {
                let target = (*sc).u.number;
                for i in 0..(*set).count {
                    if mkr_limit_eval_op(limits, err) != 0 {
                        return None;
                    }
                    let s = cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?;
                    if (bytes_to_number(s) == target) == want_eq {
                        return Some(true);
                    }
                }
                Some(false)
            }
            T_BOOLEAN => {
                let eq = ((*set).count > 0) == ((*sc).u.boolean != 0);
                Some(if want_eq { eq } else { !eq })
            }
            _ => {
                let mut target = Text::new();
                if !val_to_owned_text_or_fail::<D>(sc, limits, err, target.as_mut()) {
                    return None;
                }
                let want = target.as_slice();
                for i in 0..(*set).count {
                    if mkr_limit_eval_op(limits, err) != 0 {
                        return None;
                    }
                    let s = cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?;
                    if (s == want) == want_eq {
                        return Some(true);
                    }
                }
                Some(false)
            }
        }
    } else if lt == T_BOOLEAN || rt == T_BOOLEAN {
        let eq = val_to_boolean(l) == val_to_boolean(r);
        Some(if want_eq { eq } else { !eq })
    } else if lt == T_NUMBER || rt == T_NUMBER {
        /* Both operands are non-node-sets here, so the unchecked coercion is the
         * right entry - it cannot allocate. */
        let eq = val_to_number_unchecked::<D>(l) == val_to_number_unchecked::<D>(r);
        Some(if want_eq { eq } else { !eq })
    } else {
        let mut ls = Text::new();
        let mut rs = Text::new();
        if !val_to_owned_text_or_fail::<D>(l, limits, err, ls.as_mut())
            || !val_to_owned_text_or_fail::<D>(r, limits, err, rs.as_mut())
        {
            return None;
        }
        let eq = ls.as_slice() == rs.as_slice();
        Some(if want_eq { eq } else { !eq })
    }
}

fn rel_hit(op: u32, a: f64, b: f64) -> bool {
    match op {
        OP_LT => a < b,
        OP_LE => a <= b,
        OP_GT => a > b,
        OP_GE => a >= b,
        _ => false,
    }
}

/// §3.4 relational. A node-set on either side is true iff SOME pair satisfies
/// the relation on their numeric string-values - every pair, not just the first
/// node of each side.
unsafe fn compare_rel<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    op: u32,
    err: *mut Error,
) -> Option<bool> {
    let limits = mkr_ctx_limits(ctx);
    let (lt, rt) = ((*l).type_, (*r).type_);

    if lt == T_NODESET && rt == T_NODESET {
        let (ls, rs) = (&raw const (*l).u.nodeset, &raw const (*r).u.nodeset);
        for i in 0..(*ls).count {
            let a = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(ls, i), err)?);
            for j in 0..(*rs).count {
                if mkr_limit_eval_op(limits, err) != 0 {
                    return None;
                }
                let b = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(rs, j), err)?);
                if rel_hit(op, a, b) {
                    return Some(true);
                }
            }
        }
        return Some(false);
    }
    if lt == T_NODESET || rt == T_NODESET {
        let (ns, sc) = if lt == T_NODESET { (l, r) } else { (r, l) };
        let swap = lt != T_NODESET;
        let mut scn = 0.0;
        if !val_to_number_or_fail::<D>(sc, limits, err, &mut scn) {
            return None;
        }
        let set = &raw const (*ns).u.nodeset;
        for i in 0..(*set).count {
            if mkr_limit_eval_op(limits, err) != 0 {
                return None;
            }
            let nv = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?);
            let (a, b) = if swap { (scn, nv) } else { (nv, scn) };
            if rel_hit(op, a, b) {
                return Some(true);
            }
        }
        return Some(false);
    }
    let (mut a, mut b) = (0.0, 0.0);
    if !val_to_number_or_fail::<D>(l, limits, err, &mut a)
        || !val_to_number_or_fail::<D>(r, limits, err, &mut b)
    {
        return None;
    }
    Some(rel_hit(op, a, b))
}

/* ---------- union ---------- */

unsafe fn union_nodeset<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    if (*l).type_ != T_NODESET || (*r).type_ != T_NODESET {
        err_setf!(err, XP_ERR_TYPE, "operands of '|' must be node-sets");
        return false;
    }
    let limits = mkr_ctx_limits(ctx);
    /* Push both sides without deduplicating per insert - that was quadratic -
     * then sort once and collapse adjacent duplicates. */
    let mut merged = Set::new();
    for side in [l, r] {
        let set = &raw const (*side).u.nodeset;
        for i in 0..(*set).count {
            if !merged.push::<D>(nodeset_at::<D>(set, i), limits, err) {
                return false;
            }
        }
    }
    /* §3.3: the result of '|' is a node-set in document order, which the
     * downstream string() / number() / positional predicates assume. */
    nodeset_unique_sorted::<D>(ctx, merged.as_mut());
    (*out).type_ = T_NODESET;
    (*out).u.nodeset = merged.take();
    true
}

/* ---------- the at_xpath first-match short-circuit ---------- */

/// `Node#at_xpath` wants only the first node in document order, and today builds
/// the whole node-set to take [0]. For the common "find a descendant by name
/// (plus a simple attribute predicate)" shapes the subtree can be walked in
/// document order and stopped at the first match - the XPath-side analogue of
/// at_css's MATCH_FIRST.
///
/// Recognised, after the parser's `//` peephole:
///
///     //X        .//X          -> PATH [ {descendant, X} ]
///     //X[@a..]  .//X[@a..]    -> PATH [ {desc-or-self, node()}, {child, X, preds} ]
///     descendant::X[@a..]      -> PATH [ {descendant, X, preds} ]
///
/// where every predicate is a position-independent `[@name]` / `[@name='lit']`.
/// Each denotes "the strict descendants of the start node matching the test and
/// predicates, in document order", so the first node the pre-order walk reaches
/// IS node-set[0] of the full evaluation - identical, just without building the
/// rest. Anything else returns None and the caller runs the full evaluator.
unsafe fn first_recognise(ast: *const Node) -> Option<*const Step> {
    if ast.is_null() || (*ast).kind != NK_PATH {
        return None;
    }
    let steps = (*ast).u.path.steps;
    let nsteps = (*ast).u.path.nsteps;
    let nt: *const Step = if nsteps == 1 && (*steps).axis == AXIS_DESCENDANT {
        steps
    } else if nsteps == 2
        && (*steps).axis == AXIS_DESCENDANT_OR_SELF
        && (*steps).test.kind == NT_NODE
        && (*steps).npredicates == 0
        && (*steps.add(1)).axis == AXIS_CHILD
    {
        steps.add(1)
    } else {
        return None;
    };
    /* A prefixed name test is allowed - the caller reproduces the step driver's
     * "unknown prefix is a RUNTIME error" first, and the name match resolves the
     * prefix exactly as the full evaluator does. A prefixed ATTRIBUTE predicate
     * still falls back: match_attr_step requires an unprefixed @name. */
    for &p in step_preds(nt) {
        match_attr_pred(p)?;
    }
    Some(nt)
}

/// Does `n` satisfy every already-recognised attribute predicate of `step`?
unsafe fn first_node_ok<D: Dom>(step: *const Step, n: D::Node) -> bool {
    for &p in step_preds(step) {
        /* The recogniser already confirmed the shape. */
        let ap = match match_attr_pred(p) {
            Some(ap) => ap,
            None => return false,
        };
        if !attr_pred_matches::<D>(&ap, n) {
            return false;
        }
    }
    true
}

/// Walk for the first match if `ast` is a recognised shape.
///
/// Returns Ok(Some(node)) or Ok(Some(null)) when it handled the expression,
/// Ok(None) when the shape is not recognised, and Err(()) when the op budget was
/// exceeded. Every visited node is charged, so a huge late- or no-match document
/// fails closed here exactly as it would in the full evaluator.
///
/// # Safety
/// `ctx` must be the evaluating context and `ast` a live AST.
pub unsafe fn try_first_match<D: Dom>(
    ctx: *mut Context,
    ast: *const Node,
    err: *mut Error,
) -> Result<Option<D::Node>, ()> {
    let step = match first_recognise(ast) {
        Some(s) => s,
        None => return Ok(None),
    };
    let test = &raw const (*step).test;

    /* Reproduce the step driver's prefix validation, so the fast path stays
     * identical to the full evaluator down to the errors. */
    if !(*test).prefix.ptr.is_null() && lookup_ns(ctx, owned_bytes((*test).prefix)).is_none() {
        err_setf!(
            err,
            XP_ERR_RUNTIME,
            "unknown namespace prefix '{}' in name test",
            Bytes(owned_bytes((*test).prefix))
        );
        return Err(());
    }

    let start: D::Node = if (*ast).u.path.absolute != 0 {
        void_node::<D>(mkr_ctx_document(ctx))
    } else {
        void_node::<D>(mkr_ctx_node(ctx))
    };
    if D::is_null(start) {
        return Ok(Some(D::null())); /* recognised; no context means no match */
    }

    let limits = mkr_ctx_limits(ctx);
    let b = Bindings::<D>::new(ctx, None);
    let mut n = D::first_child(start);
    while !D::is_null(n) {
        if mkr_limit_eval_op(limits, err) != 0 {
            return Err(());
        }
        if node_principal_match::<D>(test, n, (*step).axis, &b) && first_node_ok::<D>(step, n)
        {
            return Ok(Some(n));
        }
        if !D::is_null(D::first_child(n)) {
            n = D::first_child(n);
            continue;
        }
        while n != start && D::is_null(D::next(n)) {
            n = D::parent(n);
        }
        if n == start {
            break;
        }
        n = D::next(n);
    }
    Ok(Some(D::null()))
}

/* ---------- the expression evaluator ---------- */

unsafe fn eval_path<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    self_node: D::Node,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    let mut seed = Set::new();
    if (*n).u.path.absolute != 0 {
        let root = mkr_ctx_document(ctx);
        if root.is_null() {
            err_setf!(err, XP_ERR_RUNTIME, "absolute path with no document");
            return false;
        }
        if !seed.push::<D>(void_node::<D>(root), limits, err) {
            return false;
        }
    } else if !seed.push::<D>(self_node, limits, err) {
        return false;
    }
    eval_steps::<D>(ctx, path_steps((*n).u.path.steps, (*n).u.path.nsteps), &mut seed, out, err)
}

unsafe fn eval_filter<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let f = &raw const (*n).u.filter;
    let mut primary = OwnedVal::new();
    if !eval_node::<D>(ctx, (*f).expr, focus, primary.as_mut(), err) {
        return false;
    }
    if (*f).npreds > 0 {
        if (*primary.as_ptr()).type_ != T_NODESET {
            err_setf!(err, XP_ERR_TYPE, "predicate applied to non-node-set");
            return false;
        }
        let mut set = Set::adopt((*primary.as_ptr()).u.nodeset);
        (*primary.as_mut()).u.nodeset = NodeSet { items: ptr::null_mut(), count: 0, capacity: 0 };
        let preds = core::slice::from_raw_parts((*f).preds, (*f).npreds);
        if !apply_predicates::<D>(ctx, preds, &mut set, err) {
            return false;
        }
        (*primary.as_mut()).u.nodeset = set.take();
    }
    if (*f).npath > 0 {
        if (*primary.as_ptr()).type_ != T_NODESET {
            err_setf!(err, XP_ERR_TYPE, "path applied to non-node-set");
            return false;
        }
        let mut seed = Set::adopt((*primary.as_ptr()).u.nodeset);
        (*primary.as_mut()).u.nodeset = NodeSet { items: ptr::null_mut(), count: 0, capacity: 0 };
        return eval_steps::<D>(ctx, path_steps((*f).path_steps, (*f).npath), &mut seed, out, err);
    }
    *out = primary.take();
    true
}

unsafe fn eval_fncall<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let call = &raw const (*n).u.fncall;
    let prefix = owned_bytes((*call).prefix);
    let name = owned_bytes((*call).name);

    let ns_uri: Option<&[u8]> = if (*call).prefix.ptr.is_null() {
        None
    } else {
        match lookup_ns(ctx, prefix) {
            Some(u) => Some(u),
            None => {
                err_setf!(err, XP_ERR_RUNTIME, "unknown namespace prefix '{}'", Bytes(prefix));
                return false;
            }
        }
    };
    let builtin = funcs::lookup::<D>(ns_uri, name);

    /* The arguments are evaluated once and reused by either path. Their values
     * are owned here and cleared on the way out. */
    let nargs = (*call).nargs;
    let mut args: Vec<Val> = Vec::new();
    if nargs > 0 {
        if args.try_reserve_exact(nargs).is_err() {
            err_setf!(err, XP_ERR_OOM, "out of memory allocating function arguments");
            return false;
        }
        for i in 0..nargs {
            let mut v = val_zero(T_NODESET);
            if !eval_node::<D>(ctx, *(*call).args.add(i), focus, &mut v, err) {
                mkr_val_clear(&mut v);
                clear_args(&mut args);
                return false;
            }
            args.push(v);
        }
    }

    let ok = if let Some(f) = builtin {
        f(ctx, focus, &args, out, err)
    } else {
        /* No built-in. Delegate to the per-call resolver, which the Ruby handler
         * bridge installs for the duration of evaluate(). */
        let resolved = match mkr_ctx_func_resolver(ctx) {
            Some(resolver) => resolver(
                mkr_xpath_get_user_data(ctx),
                ctx,
                node_void::<D>(focus.node),
                focus.pos,
                focus.size,
                ns_uri.map_or(ptr::null(), |u| u.as_ptr() as *const c_char),
                (*call).name.ptr,
                args.as_mut_ptr() as *mut c_void,
                nargs,
                out as *mut c_void,
                err,
            ),
            None => 1, /* not found */
        };
        if resolved > 0 {
            err_setf!(
                err,
                XP_ERR_RUNTIME,
                "unknown function {}{}{}",
                Bytes(prefix),
                if (*call).prefix.ptr.is_null() { "" } else { ":" },
                Bytes(name)
            );
            false
        } else {
            resolved == 0
        }
    };
    clear_args(&mut args);
    ok
}

unsafe fn clear_args(args: &mut Vec<Val>) {
    for v in args.iter_mut() {
        mkr_val_clear(v);
    }
    args.clear();
}

unsafe fn eval_binop<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let b = &raw const (*n).u.binop;
    let op = (*b).op;
    let limits = mkr_ctx_limits(ctx);

    /* and / or short-circuit. */
    if op == OP_OR || op == OP_AND {
        let mut l = OwnedVal::new();
        if !eval_node::<D>(ctx, (*b).lhs, focus, l.as_mut(), err) {
            return false;
        }
        let lb = val_to_boolean(l.as_ptr());
        if (op == OP_OR && lb) || (op == OP_AND && !lb) {
            *out = val_boolean(lb);
            return true;
        }
        let mut r = OwnedVal::new();
        if !eval_node::<D>(ctx, (*b).rhs, focus, r.as_mut(), err) {
            return false;
        }
        *out = val_boolean(val_to_boolean(r.as_ptr()));
        return true;
    }

    let mut l = OwnedVal::new();
    let mut r = OwnedVal::new();
    if !eval_node::<D>(ctx, (*b).lhs, focus, l.as_mut(), err)
        || !eval_node::<D>(ctx, (*b).rhs, focus, r.as_mut(), err)
    {
        return false;
    }
    let (lp, rp) = (l.as_ptr(), r.as_ptr());

    match op {
        OP_EQ | OP_NE => match compare_eq::<D>(ctx, lp, rp, op, err) {
            Some(v) => {
                *out = val_boolean(v);
                true
            }
            None => false,
        },
        OP_LT | OP_LE | OP_GT | OP_GE => match compare_rel::<D>(ctx, lp, rp, op, err) {
            Some(v) => {
                *out = val_boolean(v);
                true
            }
            None => false,
        },
        OP_ADD | OP_SUB | OP_MUL | OP_DIV | OP_MOD => {
            let (mut a, mut c) = (0.0, 0.0);
            if !val_to_number_or_fail::<D>(lp, limits, err, &mut a)
                || !val_to_number_or_fail::<D>(rp, limits, err, &mut c)
            {
                return false;
            }
            *out = val_number(match op {
                OP_ADD => a + c,
                OP_SUB => a - c,
                OP_MUL => a * c,
                OP_DIV => a / c,
                _ => libm_fmod(a, c),
            });
            true
        }
        /* union_nodeset reports its own typed error; do not overwrite it. */
        OP_UNION => union_nodeset::<D>(ctx, lp, rp, out, err),
        _ => {
            err_setf!(err, XP_ERR_INTERNAL, "unexpected binop");
            false
        }
    }
}

/// The evaluator's only recursive function, and therefore the whole of "AST
/// recursion is bounded": one op and one recursion level are charged on entry
/// and the level is released at the single exit. Keeping it single-exit is what
/// makes that balance locally checkable.
unsafe fn eval_node<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    if mkr_limit_eval_op(limits, err) != 0 {
        return false;
    }
    if mkr_limit_recurse_enter(limits, err) != 0 {
        return false;
    }
    let ok = eval_node_inner::<D>(ctx, n, focus, out, err);
    mkr_limit_recurse_leave(limits);
    ok
}

unsafe fn eval_node_inner<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);

    /* Hoisting: a context-independent subtree already computed in this evaluate
     * comes back as a clone, which keeps ownership clean - clearing either copy
     * is safe. */
    if (*n).is_context_independent != 0 && (*n).memoized != 0 {
        return val_clone(&raw const (*n).memo_value, out, err);
    }

    let ok = match (*n).kind {
        NK_LITERAL_STR => {
            let mut text = OwnedText { ptr: ptr::null_mut(), len: 0 };
            if owned_copy(&mut text, owned_bytes((*n).u.literal), err, b"out of memory copying literal\0")
            {
                mkr_val_set_owned_text(out, text);
                true
            } else {
                false
            }
        }
        NK_LITERAL_NUM => {
            *out = val_number((*n).u.literal_num);
            true
        }
        NK_VARREF => {
            let v = &raw const (*n).u.varref;
            let mut got = VerifiedText { ptr: ptr::null(), len: 0 };
            if mkr_ctx_lookup_variable_text(
                ctx,
                (*v).prefix.ptr,
                (*v).prefix.len,
                (*v).name.ptr,
                (*v).name.len,
                &mut got,
            ) == 0
            {
                err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "undefined variable ${}{}{}",
                    Bytes(owned_bytes((*v).prefix)),
                    if (*v).prefix.ptr.is_null() { "" } else { ":" },
                    Bytes(owned_bytes((*v).name))
                );
                false
            } else {
                let bytes = if got.ptr.is_null() || got.len == 0 {
                    &[][..]
                } else {
                    core::slice::from_raw_parts(got.ptr as *const u8, got.len)
                };
                let mut text = OwnedText { ptr: ptr::null_mut(), len: 0 };
                if owned_copy(&mut text, bytes, err, b"out of memory copying variable value\0") {
                    mkr_val_set_owned_text(out, text);
                    true
                } else {
                    false
                }
            }
        }
        NK_FNCALL => eval_fncall::<D>(ctx, n, focus, out, err),
        NK_UNARY => {
            let mut v = OwnedVal::new();
            if !eval_node::<D>(ctx, (*n).u.unary.expr, focus, v.as_mut(), err) {
                false
            } else {
                let mut d = 0.0;
                if val_to_number_or_fail::<D>(v.as_ptr(), limits, err, &mut d) {
                    *out = val_number(-d);
                    true
                } else {
                    false
                }
            }
        }
        NK_BINOP => eval_binop::<D>(ctx, n, focus, out, err),
        NK_PATH => eval_path::<D>(ctx, n, focus.node, out, err),
        NK_FILTER => eval_filter::<D>(ctx, n, focus, out, err),
        _ => {
            err_setf!(err, XP_ERR_INTERNAL, "unknown AST node");
            false
        }
    };

    /* Memoize a context-independent subtree on success. The clone keeps the
     * caller's value independent of the cached one, which matters because the
     * caller is free to consume theirs. */
    if ok && (*n).is_context_independent != 0 && (*n).memoized == 0 {
        let mut memo = val_zero(T_NODESET);
        if val_clone(out, &mut memo, err) {
            /* The AST is read-only at eval time apart from these memo slots. */
            let mut_n = n as *mut Node;
            (*mut_n).memo_value = memo;
            (*mut_n).memoized = 1;
        } else {
            /* OOM during the clone: the caller's `out` is still valid, so leave
             * the node unmemoized and surface the error. */
            return false;
        }
    }
    ok
}

/// Evaluate an AST against the context, with the context node as the focus.
///
/// # Safety
/// `ctx` must be a live context and `ast` a live AST built by `mkr_parse`.
pub unsafe fn eval_ast<D: Dom>(
    ctx: *mut Context,
    ast: *const Node,
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let focus = Focus::<D> { node: void_node::<D>(mkr_ctx_node(ctx)), pos: 1, size: 1 };
    eval_node::<D>(ctx, ast, &focus, out, err)
}

extern "C" {
    #[link_name = "fmod"]
    fn libm_fmod(a: f64, b: f64) -> f64;
}
