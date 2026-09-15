//! Node tests: does this node match a step's test?
//!
//! Its own module because both callers of a name test live elsewhere - the step
//! driver in eval.rs, and the element-index fast paths in step_index.rs, which
//! re-check a bucket candidate against the same test. Leaving it in eval.rs made
//! those two modules depend on each other.

use super::abi::*;
use super::dom::*;

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
    pub cx: &'a Context,
    pub doc: D::Doc,
    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub lax: bool,
    /// The name test's prefix, already resolved to a URI.
    pub pre: Option<&'a [u8]>,
}

impl<'a, D: Dom> Bindings<'a, D> {
    pub fn new(cx: &'a Context, doc: D::Doc, pre: Option<&'a [u8]>) -> Bindings<'a, D> {
        Bindings {
            cx,
            doc,
            lax: cx.lax(),
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
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<D>,
) -> bool {
    let Some(want_local) = test.local.as_deref() else {
        return false;
    };
    let is_attr = axis == Axis::Attribute;
    let prefixed = test.prefix.is_some();

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

unsafe fn resolved_prefix<'a, D: Dom>(b: &Bindings<'a, D>, test: &NodeTest) -> Option<&'a [u8]> {
    match b.pre {
        Some(u) => Some(u),
        None => b.cx.lookup_ns(test.prefix.as_deref().unwrap_or(&[])),
    }
}

///
/// # Safety
/// `node` must be a live handle, and `b` bindings built for the evaluating
/// context.
pub unsafe fn node_principal_match<D: Dom>(
    doc: D::Doc,
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<D>,
) -> bool {
    match test.kind {
        TestKind::Node => {
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
        TestKind::Text => matches!(D::node_type(doc, node), NTYPE_TEXT | NTYPE_CDATA_SECTION),
        TestKind::Comment => D::node_type(doc, node) == NTYPE_COMMENT,
        TestKind::Pi => {
            if D::node_type(doc, node) != NTYPE_PI {
                return false;
            }
            match test.pi_target.as_deref() {
                None => true,
                Some(target) => D::pi_name(doc, node) == target,
            }
        }
        TestKind::Wildcard => {
            if axis == Axis::Namespace {
                return false;
            }
            /* the principal node type of the axis */
            if axis == Axis::Attribute {
                if D::node_type(doc, node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(doc, node) != NTYPE_ELEMENT {
                return false;
            }
            /* `*` matches any namespace; `prefix:*` only the one bound to the
             * prefix. An unknown prefix is reported up front by the step driver;
             * here it is a non-match. */
            if test.prefix.is_none() {
                return true;
            }
            match resolved_prefix(b, test) {
                Some(want) => want == D::ns_uri(b.doc, node),
                None => false,
            }
        }
        TestKind::Name => {
            if axis == Axis::Attribute {
                if D::node_type(doc, node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if D::node_type(doc, node) != NTYPE_ELEMENT {
                return false;
            }
            name_test_match::<D>(doc, test, node, axis, b)
        }
    }
}
