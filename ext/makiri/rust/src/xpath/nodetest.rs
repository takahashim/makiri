//! Node tests: does this node match a step's test?
//!
//! Its own module because both callers of a name test live elsewhere - the step
//! driver in eval.rs, and the element-index fast paths in step_index.rs, which
//! re-check a bucket candidate against the same test. Leaving it in eval.rs made
//! those two modules depend on each other.

#![forbid(unsafe_code)]

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
pub struct Bindings<'a, D: Dom<'a>> {
    pub cx: &'a Context,
    pub doc: D,
    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub lax: bool,
    /// The name test's prefix, already resolved to a URI.
    pub pre: Option<&'a [u8]>,
}

impl<'a, D: Dom<'a>> Bindings<'a, D> {
    pub fn new(cx: &'a Context, doc: D, pre: Option<&'a [u8]>) -> Bindings<'a, D> {
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
///
/// The caller has checked an element's kind; an attribute is checked here, by
/// taking it as one.
fn name_test_match<'a, D: Dom<'a>>(
    doc: D,
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<'a, D>,
) -> bool {
    let Some(want_local) = test.local.as_deref() else {
        return false;
    };
    let is_attr = axis == Axis::Attribute;
    let prefixed = test.prefix.is_some();

    let got: &[u8] = if is_attr {
        let Some(a) = doc.as_attr(node) else {
            return false;
        };
        if D::IS_XML || prefixed {
            doc.attr_local_name(a)
        } else {
            doc.attr_qualified_name(a)
        }
    } else if D::IS_XML || prefixed {
        doc.local_name(node)
    } else {
        doc.qualified_name(node)
    };
    if got != want_local {
        return false;
    }

    if prefixed {
        let want_uri = match resolved_prefix(b, test) {
            Some(u) => u,
            None => return false, /* unknown prefix -> non-match; the step driver reports it */
        };
        return want_uri == b.doc.ns_uri(node);
    }
    if b.lax {
        return true;
    }
    if D::IS_XML {
        /* strict unprefixed: the node must be in no namespace */
        b.doc.ns_uri(node).is_empty()
    } else {
        /* strict: unprefixed ELEMENT tests resolve in the HTML namespace, so a
         * foreign (SVG / MathML) element needs a prefix. Attributes are exempt -
         * an unprefixed attribute test matches by no-namespace local name, and
         * the qualified-name compare above already excluded prefixed foreign
         * attributes. */
        is_attr || !doc.is_foreign_ns(node)
    }
}

fn resolved_prefix<'a, D: Dom<'a>>(b: &Bindings<'a, D>, test: &NodeTest) -> Option<&'a [u8]> {
    match b.pre {
        Some(u) => Some(u),
        None => b.cx.lookup_ns(test.prefix.as_deref().unwrap_or(&[])),
    }
}

/// Whether `node` passes `test` on `axis`, with `b` built for the evaluating
/// context.
pub fn node_principal_match<'a, D: Dom<'a>>(
    doc: D,
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<'a, D>,
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
                doc.node_type(node),
                NTYPE_DOCUMENT_TYPE | NTYPE_ENTITY | NTYPE_ENTITY_REFERENCE | NTYPE_NOTATION
            )
        }
        TestKind::Text => matches!(doc.node_type(node), NTYPE_TEXT | NTYPE_CDATA_SECTION),
        TestKind::Comment => doc.node_type(node) == NTYPE_COMMENT,
        TestKind::Pi => {
            if doc.node_type(node) != NTYPE_PI {
                return false;
            }
            match test.pi_target.as_deref() {
                None => true,
                Some(target) => doc.pi_name(node) == target,
            }
        }
        TestKind::Wildcard => {
            if axis == Axis::Namespace {
                return false;
            }
            /* the principal node type of the axis */
            if axis == Axis::Attribute {
                if doc.node_type(node) != NTYPE_ATTRIBUTE {
                    return false;
                }
            } else if doc.node_type(node) != NTYPE_ELEMENT {
                return false;
            }
            /* `*` matches any namespace; `prefix:*` only the one bound to the
             * prefix. An unknown prefix is reported up front by the step driver;
             * here it is a non-match. */
            if test.prefix.is_none() {
                return true;
            }
            match resolved_prefix(b, test) {
                Some(want) => want == b.doc.ns_uri(node),
                None => false,
            }
        }
        TestKind::Name => {
            /* An attribute's kind is checked by the name test, which takes it as
             * one. */
            if axis != Axis::Attribute && doc.node_type(node) != NTYPE_ELEMENT {
                return false;
            }
            name_test_match::<D>(doc, test, node, axis, b)
        }
    }
}
