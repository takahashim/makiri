//! Node tests: does this node match a step's test?
//!
//! Its own module because both callers of a name test live elsewhere - the step
//! driver in eval.rs, and the element-index fast paths in step_index.rs, which
//! re-check a bucket candidate against the same [`CompiledTest`]. Leaving it in
//! eval.rs made those two modules depend on each other.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;

/// One step's node test, compiled against the evaluating context: the test,
/// the axis it runs on, its prefix resolved to a URI, and the matching mode.
///
/// Built once per step - which is where an unknown prefix is reported, as the
/// one RUNTIME error every caller used to spell for itself - and asked once per
/// visited node, the hottest loop in the engine, with nothing left to look up.
#[derive(Clone, Copy)]
pub struct CompiledTest<'a> {
    test: &'a NodeTest,
    axis: Axis,
    /// The URI the test's prefix is bound to; None for an unprefixed test.
    uri: Option<&'a [u8]>,
    /// namespace_matching: :lax.
    lax: bool,
}

impl<'a> CompiledTest<'a> {
    /// `test` on `axis`, its prefix resolved through `names`. `Err` - a RUNTIME
    /// error, written to `err` - for a prefix `names` does not bind: a uniform
    /// error rather than a silently empty match.
    pub fn new(
        test: &'a NodeTest,
        axis: Axis,
        names: &'a Names,
        lax: bool,
        err: ErrSink,
    ) -> Result<CompiledTest<'a>, Reported> {
        let uri = match test.prefix.as_deref() {
            None => None,
            Some(prefix) => Some(names.resolve_prefix(prefix, err)?),
        };
        Ok(CompiledTest {
            test,
            axis,
            uri,
            lax,
        })
    }

    pub fn test(&self) -> &'a NodeTest {
        self.test
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// The URI the prefix resolved to; None for an unprefixed test.
    pub fn uri(&self) -> Option<&'a [u8]> {
        self.uri
    }

    pub fn lax(&self) -> bool {
        self.lax
    }

    /// Whether `node` passes the test.
    #[inline]
    pub fn matches<'d, D: Dom<'d>>(&self, doc: D, node: D::Node) -> bool {
        let (test, axis) = (self.test, self.axis);
        match test.kind {
            TestKind::Node => {
                /* §5's data model has only element, attribute, text, namespace,
                 * PI, comment and the root. Both representations additionally
                 * carry DOCUMENT_TYPE / ENTITY / ENTITY_REFERENCE / NOTATION
                 * nodes, which are not in the model, so node() must not match
                 * them. A DocumentFragment IS matched: it is the root of a
                 * fragment-rooted context, so '.' over a fragment has to see it. */
                !matches!(
                    doc.node_type(node),
                    NTYPE_DOCUMENT_TYPE | NTYPE_ENTITY | NTYPE_ENTITY_REFERENCE | NTYPE_NOTATION
                )
            }
            TestKind::Text => matches!(doc.node_type(node), NTYPE_TEXT | NTYPE_CDATA_SECTION),
            TestKind::Comment => doc.node_type(node) == NTYPE_COMMENT,
            TestKind::Pi => {
                doc.node_type(node) == NTYPE_PI
                    && test
                        .pi_target
                        .as_deref()
                        .is_none_or(|target| doc.pi_name(node) == target)
            }
            TestKind::Wildcard => {
                if axis == Axis::Namespace {
                    return false;
                }
                /* The principal node type of the axis. `*` matches any
                 * namespace; `prefix:*` only the one bound to the prefix. */
                if axis == Axis::Attribute {
                    let Some(a) = doc.as_attr(node) else {
                        return false;
                    };
                    self.uri.is_none_or(|want| want == doc.attr_ns_uri(a))
                } else {
                    doc.node_type(node) == NTYPE_ELEMENT
                        && self.uri.is_none_or(|want| want == doc.ns_uri(node))
                }
            }
            TestKind::Name => {
                /* An attribute's kind is checked by the name test, which takes
                 * it as one. */
                if axis != Axis::Attribute && doc.node_type(node) != NTYPE_ELEMENT {
                    return false;
                }
                self.name_matches(doc, node)
            }
        }
    }

    /// The element / attribute name match. The principal-node-type filter has
    /// already passed; this decides name and namespace, through the host's
    /// policy items (`Dom::test_name`, `unprefixed_matches`, `attr_ns_uri`).
    ///
    /// The caller has checked an element's kind; an attribute is checked here,
    /// by taking it as one.
    fn name_matches<'d, D: Dom<'d>>(&self, doc: D, node: D::Node) -> bool {
        let Some(want_local) = self.test.local.as_deref() else {
            return false;
        };
        if self.axis == Axis::Attribute {
            let Some(a) = doc.as_attr(node) else {
                return false;
            };
            return match self.uri {
                None => unprefixed_attr_matches(doc, doc.parent(node), a, want_local, self.lax),
                Some(uri) => {
                    names_equal(
                        doc,
                        doc.parent(node),
                        doc.attr_test_name(a, true),
                        want_local,
                    ) && uri == doc.attr_ns_uri(a)
                }
            };
        }

        let prefixed = self.uri.is_some();
        names_equal(doc, Some(node), doc.test_name(node, prefixed), want_local)
            && match self.uri {
                Some(uri) => uri == doc.ns_uri(node),
                None => doc.unprefixed_matches(node, false, self.lax),
            }
    }
}

/// Whether attribute `a` of element `owner` matches the unprefixed name test
/// `want`. The attribute axis and the `[@name]` fast path both ask this, so the
/// two cannot answer differently - they once did, in XML's lax mode.
///
/// `owner` is the attribute's element (its parent in both backends), which
/// decides whether names fold case.
pub fn unprefixed_attr_matches<'d, D: Dom<'d>>(
    doc: D,
    owner: Option<D::Node>,
    a: D::Attr,
    want: &[u8],
    lax: bool,
) -> bool {
    names_equal(doc, owner, doc.attr_test_name(a, false), want)
        && doc.unprefixed_matches(D::attr_node(a), true, lax)
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
