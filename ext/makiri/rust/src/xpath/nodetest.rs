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
pub struct Bindings<'a, 'd, D: Dom<'d>> {
    pub cx: &'a Context<'d, D>,
    /// The context's registrations, for a prefix the step did not resolve.
    pub names: &'a Names,
    pub doc: D,
    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub lax: bool,
    /// The name test's prefix, already resolved to a URI.
    pub pre: Option<&'a [u8]>,
}

impl<'a, 'd, D: Dom<'d>> Bindings<'a, 'd, D> {
    pub fn new(
        cx: &'a Context<'d, D>,
        names: &'a Names,
        doc: D,
        pre: Option<&'a [u8]>,
    ) -> Bindings<'a, 'd, D> {
        Bindings {
            cx,
            names,
            doc,
            lax: cx.lax(),
            pre,
        }
    }
}

/// The element / attribute name match. The principal-node-type filter has
/// already passed; this decides name and namespace, through the host's policy
/// items (`Dom::test_name`, `unprefixed_matches`, `attr_ns_uri`).
///
/// `pre` carries the prefix's already-resolved URI, so a hot multi-node walk
/// resolves it once in `eval_step` rather than per node.
///
/// The caller has checked an element's kind; an attribute is checked here, by
/// taking it as one.
fn name_test_match<'a, 'd, D: Dom<'d>>(
    doc: D,
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<'a, 'd, D>,
) -> bool {
    let Some(want_local) = test.local.as_deref() else {
        return false;
    };
    if axis == Axis::Attribute {
        let Some(a) = doc.as_attr(node) else {
            return false;
        };
        return match test.prefix {
            None => unprefixed_attr_matches(doc, doc.parent(node), a, want_local, b.lax),
            Some(_) => {
                names_equal(
                    doc,
                    doc.parent(node),
                    doc.attr_test_name(a, true),
                    want_local,
                ) && resolved_prefix(b, test).is_some_and(|uri| uri == doc.attr_ns_uri(a))
            }
        };
    }

    let prefixed = test.prefix.is_some();
    if !names_equal(doc, Some(node), doc.test_name(node, prefixed), want_local) {
        return false;
    }
    if prefixed {
        /* An unknown prefix is a non-match here; the step driver reports it. */
        return resolved_prefix(b, test).is_some_and(|uri| uri == doc.ns_uri(node));
    }
    b.lax || doc.unprefixed_matches(node, false)
}

/// Whether attribute `a` of element `owner` matches the unprefixed name test
/// `want`. The attribute axis and the `[@name]` fast path both ask this, so the
/// two cannot answer differently - they once did, in XML's lax mode, where the
/// fast path compared qualified names and the axis local ones.
///
/// `owner` is the attribute's element, which decides whether names fold case;
/// the HTML index backfills it as the attribute's parent.
pub fn unprefixed_attr_matches<'d, D: Dom<'d>>(
    doc: D,
    owner: Option<D::Node>,
    a: D::Attr,
    want: &[u8],
    lax: bool,
) -> bool {
    names_equal(doc, owner, doc.attr_test_name(a, false), want)
        && (lax || doc.unprefixed_matches(D::attr_node(a), true))
}

/// A name test's name against a node's, as the node's element `owner` decides:
/// ASCII case-insensitively for an HTML element in an HTML document (so
/// `//DiV` finds `<div>` and `[@Id]` its `id`, while `Ø` still differs from
/// `ø`), byte for byte otherwise. Shared with the `[@name]` fast path, which has
/// to agree with the attribute axis node for node.
pub fn names_equal<'d, D: Dom<'d>>(
    doc: D,
    owner: Option<D::Node>,
    got: &[u8],
    want: &[u8],
) -> bool {
    if owner.is_some_and(|el| doc.folds_name_case(el)) {
        got.eq_ignore_ascii_case(want)
    } else {
        got == want
    }
}

fn resolved_prefix<'a, 'd, D: Dom<'d>>(
    b: &Bindings<'a, 'd, D>,
    test: &NodeTest,
) -> Option<&'a [u8]> {
    match b.pre {
        Some(u) => Some(u),
        None => b.names.lookup_ns(test.prefix.as_deref().unwrap_or(&[])),
    }
}

/// Whether `node` passes `test` on `axis`, with `b` built for the evaluating
/// context.
pub fn node_principal_match<'a, 'd, D: Dom<'d>>(
    doc: D,
    test: &NodeTest,
    node: D::Node,
    axis: Axis,
    b: &Bindings<'a, 'd, D>,
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
            let got = match doc.as_attr(node) {
                Some(a) => doc.attr_ns_uri(a),
                None => doc.ns_uri(node),
            };
            resolved_prefix(b, test).is_some_and(|want| want == got)
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
