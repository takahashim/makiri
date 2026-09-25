//! The key that makes two attributes equal: a namespace URI and a local name.
//!
//! §3's uniqueness rule, in one place. The parser enforces it over the
//! attributes it has just built ([`has_duplicate`]); the mutation API over the
//! attributes already on an element ([`key_taken`]) and over a resolution's
//! pending set ([`keys_repeat`]). The raw qualified name is the DOM's OTHER
//! key for the same node, so it hangs off [`AttrKey`] too.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::{Document, NodeFlags, NodeId, Span};

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
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
fn attr_matches_ns(doc: &Document, a: NodeId, ns: &[u8], local: &[u8]) -> bool {
    /* A pending attribute's namespace is undecided, not empty: it has no key
     * to match (`set_attribute_ns("", "a")` used to overwrite a pending p:a). */
    !doc.node(a).flags.contains(NodeFlags::NS_PENDING)
        && doc.node(a).ns_uri.len as usize == ns.len()
        && (ns.is_empty() || doc.ns(a) == ns)
        && doc.local(a) == local
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
    let mut ids: Vec<NodeId> = Vec::new();
    ids.falloc_reserve_exact(count).ok()?;
    ids.extend(attrs());
    let key = |x: NodeId| (doc.ns(x), doc.local(x));
    ids.sort_unstable_by(|&x, &y| key(x).cmp(&key(y)));
    Some(ids.windows(2).any(|w| key(w[0]) == key(w[1])))
}
