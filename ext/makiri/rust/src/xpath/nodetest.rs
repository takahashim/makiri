//! Node tests: does this node match a step's test?
//!
//! Its own module because both callers of a name test live elsewhere - the step
//! driver in eval.rs, and the element-index fast paths in step_index.rs, which
//! re-check a bucket candidate against the same test. Leaving it in eval.rs made
//! those two modules depend on each other.

use super::abi::*;
use super::dom::*;
use super::value::owned_bytes;
use core::ffi::c_char;

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
    doc: D::Doc,
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
            D::attr_local_name(doc, node)
        } else {
            D::local_name(doc, node)
        }
    } else if is_attr {
        D::attr_qualified_name(doc, node)
    } else {
        D::qualified_name(doc, node)
    };
    if got != want_local {
        return false;
    }

    if prefixed {
        let want_uri = match resolved_prefix(b, test) {
            Some(u) => u,
            None => return false, /* unknown prefix -> non-match; the step driver reports it */
        };
        return want_uri == D::ns_uri(b.doc, node);
    }
    if b.lax {
        return true;
    }
    if D::IS_XML {
        /* strict unprefixed: the node must be in no namespace */
        D::ns_uri(b.doc, node).is_empty()
    } else {
        /* strict: unprefixed ELEMENT tests resolve in the HTML namespace, so a
         * foreign (SVG / MathML) element needs a prefix. Attributes are exempt -
         * an unprefixed attribute test matches by no-namespace local name, and
         * the qualified-name compare above already excluded prefixed foreign
         * attributes. */
        is_attr || !D::is_foreign_ns(doc, node)
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

///
/// # Safety
/// `ctx` must be the evaluating context. The returned bytes belong to its
/// namespace registry, which the glue refuses to re-register during an
/// evaluate - so they stay valid for the call, but not past it.
pub unsafe fn lookup_ns<'a>(ctx: *mut Context, prefix: &[u8]) -> Option<&'a [u8]> {
    let mut len = 0usize;
    let p = mkr_ctx_lookup_ns(
        ctx,
        prefix.as_ptr() as *const c_char,
        prefix.len(),
        &mut len,
    );
    if p.is_null() {
        None
    } else if len == 0 {
        Some(&[])
    } else {
        Some(core::slice::from_raw_parts(p as *const u8, len))
    }
}

///
/// # Safety
/// `test` must be a live node test in the AST being evaluated, `node` a live
/// handle, and `b` bindings built for this same context.
pub unsafe fn node_principal_match<D: Dom>(
    doc: D::Doc,
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
                D::node_type(doc, node),
                NTYPE_DOCUMENT_TYPE | NTYPE_ENTITY | NTYPE_ENTITY_REFERENCE | NTYPE_NOTATION
            )
        }
        NT_TEXT => matches!(D::node_type(doc, node), NTYPE_TEXT | NTYPE_CDATA_SECTION),
        NT_COMMENT => D::node_type(doc, node) == NTYPE_COMMENT,
        NT_PI => {
            if D::node_type(doc, node) != NTYPE_PI {
                return false;
            }
            if (*test).pi_target.ptr.is_null() {
                return true;
            }
            D::pi_name(doc, node) == owned_bytes((*test).pi_target)
        }
        NT_WILDCARD => {
            if axis == AXIS_NAMESPACE {
                return false;
            }
            /* the principal node type of the axis */
            if axis == AXIS_ATTRIBUTE {
                if D::node_type(doc, node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(doc, node) != NTYPE_ELEMENT {
                return false;
            }
            /* `*` matches any namespace; `prefix:*` only the one bound to the
             * prefix. An unknown prefix is reported up front by the step driver;
             * here it is a non-match. */
            if (*test).prefix.ptr.is_null() {
                return true;
            }
            match resolved_prefix(b, test) {
                Some(want) => want == D::ns_uri(b.doc, node),
                None => false,
            }
        }
        NT_NAME => {
            if axis == AXIS_ATTRIBUTE {
                if D::node_type(doc, node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(doc, node) != NTYPE_ELEMENT {
                return false;
            }
            name_test_match::<D>(doc, test, node, axis, b)
        }
        _ => false,
    }
}
