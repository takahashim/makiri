//! The key that makes two attributes equal: a namespace URI and a local name.
//!
//! §3's uniqueness rule, in one place. The parser enforces it over the
//! attributes it has just built ([`has_duplicate`]); the mutation API over the
//! attributes already on an element ([`key_taken`]) and over a resolution's
//! pending set ([`keys_repeat`]). The raw qualified name is the DOM's OTHER
//! key for the same node, so it hangs off [`AttrKey`] too.

#![forbid(unsafe_code)]

use crate::falloc::try_vec_with_capacity;
use crate::xml::{AttrNs, Document, NodeId, Span};

/// How an attribute is looked up: by its raw qualified name, or by the DOM's
/// `(namespace, local name)` key - the two keys for the same node.
#[derive(Clone, Copy)]
pub(crate) enum AttrKey<'a> {
    QName(&'a [u8]),
    Ns { ns: &'a [u8], local: &'a [u8] },
}

impl AttrKey<'_> {
    pub(crate) fn matches(self, doc: &Document, a: NodeId) -> bool {
        match self {
            AttrKey::QName(q) => doc.qname(a) == q,
            AttrKey::Ns { ns, local } => attr_matches_ns(doc, a, ns, local),
        }
    }

    /// The first attribute of `el` this key names - the DOM's "get an
    /// attribute by name" or "by namespace and local name" - or None, for a
    /// non-element too.
    ///
    /// Namespace declarations included: in the DOM an `xmlns` / `xmlns:p` is
    /// an attribute, so `node["xmlns:p"]` reads it as `getAttribute` does.
    /// XPath's data model is the one that hides them (`xml::xpath` skips them
    /// on the attribute axis), which is why `@xmlns:p` finds nothing while
    /// this does.
    pub(crate) fn find_in(self, doc: &Document, el: NodeId) -> Option<NodeId> {
        if doc.type_(el) != Some(crate::xml::ArenaKind::Element) {
            return None;
        }
        doc.attributes(el).find(|&a| self.matches(doc, a))
    }
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
fn attr_matches_ns(doc: &Document, a: NodeId, ns: &[u8], local: &[u8]) -> bool {
    /* A pending attribute's namespace is undecided, not empty: it has no key
     * to match (`set_attribute_ns("", "a")` used to overwrite a pending p:a). */
    doc.attr_ns_state(a) != Some(AttrNs::Pending) && doc.ns(a) == ns && doc.local(a) == local
}

/// Whether an attribute of `el` other than `except` already has the key
/// (`ns`, `local`) - the uniqueness the parser enforces (§3). Asked only for a
/// DECIDED key: a prefix not yet resolvable (a detached element) has no
/// namespace to compare, and is checked when it is.
pub(crate) fn key_taken(
    doc: &Document,
    el: NodeId,
    ns: &[u8],
    local: &[u8],
    except: Option<NodeId>,
) -> bool {
    let key = AttrKey::Ns { ns, local };
    doc.attributes(el)
        .any(|attr| Some(attr) != except && key.matches(doc, attr))
}

/// Whether two of the `(namespace, attribute)` keys are equal by namespace URI
/// and local name - the uniqueness rule (§3) a resolution checks before it
/// writes. Sorts `keys` in place.
pub(crate) fn keys_repeat(doc: &Document, keys: &mut [(Span, NodeId)]) -> bool {
    let key = |&(ns, a): &(Span, NodeId)| (doc.span(ns), doc.local(a));
    keys.sort_unstable_by(|x, y| key(x).cmp(&key(y)));
    keys.windows(2).any(|w| key(&w[0]) == key(&w[1]))
}

/// Whether two of `element`'s attributes share `(namespace URI, local
/// name)`. Whether two do - or None when the sort buffer cannot be allocated.
///
/// Pairwise for the usual handful. Past that, pairwise is quadratic in a count
/// the input picks, up to `MAX_ATTRS`: 8.4M comparisons an element, and 100
/// such elements (3.6 MB) took 16.6 s with no budget to stop it. So a longer
/// list is sorted by the pair and compared as neighbours, O(n log n).
pub(crate) fn has_duplicate(doc: &Document, element: NodeId) -> Option<bool> {
    const PAIRWISE_MAX: usize = 16;
    let attrs = || doc.attributes(element);
    let count = attrs().count();
    if count <= PAIRWISE_MAX {
        let found = attrs().enumerate().any(|(i, x)| {
            attrs()
                .skip(i + 1)
                .any(|y| doc.local(x) == doc.local(y) && doc.ns(x) == doc.ns(y))
        });
        return Some(found);
    }
    let mut ids: Vec<NodeId> = try_vec_with_capacity(count)?;
    ids.extend(attrs());
    let key = |x: NodeId| (doc.ns(x), doc.local(x));
    ids.sort_unstable_by(|&x, &y| key(x).cmp(&key(y)));
    Some(ids.windows(2).any(|w| key(w[0]) == key(w[1])))
}
