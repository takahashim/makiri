//! The per-evaluation string-value cache.

#![forbid(unsafe_code)]

use super::abi::*;
use super::value::Str;
use crate::err_setf;
use crate::falloc::Reserve;
use crate::ptr_table::PtrMap;
use crate::token::Token;

/// Where a string-value sits in the cache that returned it.
///
/// An index rather than a borrow, so a caller can hold one string-value while
/// asking the cache for the next - the node-set comparisons hold one side's
/// value across the whole scan of the other ([`NodeText`] carries it).
#[derive(Clone, Copy, Debug)]
pub struct TextId(usize);

/// One evaluate's node string-value cache: an ordered store of container
/// string-values, plus a token-keyed index into it.
///
/// An entry is the value as the walk found it: a slice of the document when
/// the container held one non-empty text (borrowed for `'d`, which the
/// document outlives the evaluate by), or the text built from several, which
/// the cache owns and drops with itself - there is nothing to clear by hand.
///
/// `max_cache_bytes` is charged each entry's LENGTH, borrowed or owned alike.
/// A borrowed entry holds no copy, but counting it keeps one rule for which
/// values stay cached, and so for when a rebuild is charged
/// ([`Budget::charge_bytes`]); counting it as free would let the cap stop
/// bounding anything a query of borrowed values does.
#[derive(Default)]
pub struct StrCache<'d> {
    entries: Vec<(Token, Str<'d>)>,
    /// node token -> entry index.
    index: PtrMap<Token, usize>,
    total_bytes: usize,
    /// The nodes whose value the cache could not keep. A node found here is
    /// being built AGAIN, which is the work [`Budget::charge_bytes`] prices.
    refused: PtrMap<Token, u8>,
}

impl<'d> StrCache<'d> {
    pub const fn new() -> StrCache<'d> {
        StrCache {
            entries: Vec::new(),
            index: PtrMap::new(),
            total_bytes: 0,
            refused: PtrMap::new(),
        }
    }

    /// The cached text of `node`, if one is.
    #[inline]
    pub fn find(&self, node: Token) -> Option<TextId> {
        self.index.get(node).map(TextId)
    }

    /// The text `id` names.
    #[inline]
    pub fn text(&self, id: TextId) -> &[u8] {
        self.entries[id.0].1.as_slice()
    }

    /// Cache `text` as `node`'s string-value, or hand it back uncached when
    /// the cache is at `budget`'s `max_cache_bytes`. Only a container's value
    /// comes here: a node that holds its own is read in place, uncached
    /// (`value::cached_node_text`).
    ///
    /// Only OOM is an error. Every refusal happens before anything is
    /// committed, so a failed insert leaves the cache as it was.
    pub fn insert(
        &mut self,
        node: Token,
        text: Str<'d>,
        budget: &mut Budget,
    ) -> Result<NodeText<'d>, Reported> {
        // Past the cap the value is still the answer, just not kept: a total
        // held to the per-string cap made `//*[. = "x"]` raise on a page where
        // `//*[string(.) = "x"]`, which never caches, answered.
        let fits = self
            .total_bytes
            .checked_add(text.as_slice().len())
            .filter(|&t| t <= budget.limits.max_cache_bytes);
        let Some(new_total) = fits else {
            /* Not kept, so a later comparison that needs it builds it again.
             * That repeated building is what is charged, by its size - from
             * the second build of a node on. A first build costs its walk,
             * cached or not: charging it priced a query by text size times
             * nesting depth. If the note cannot be made, the build is charged
             * as a repeat. */
            let again = self.refused.get(node).is_some() || self.refused.insert(node, 1).is_err();
            if again {
                budget.charge_bytes(text.as_slice().len())?;
            }
            return Ok(match text {
                Str::Borrowed(s) => NodeText::Borrowed(s),
                Str::Owned(t) => NodeText::Uncached(t),
            });
        };
        if self.entries.falloc_reserve(1).is_err() {
            return Err(err_setf!(
                budget.sink(),
                ErrorKind::Oom,
                "out of memory in node string cache"
            ));
        }

        /* Index before committing: a failed growth leaves the map as it was,
         * and the entry push below cannot fail - it was reserved above. */
        let id = self.entries.len();
        if self.index.insert(node, id).is_err() {
            return Err(err_setf!(
                budget.sink(),
                ErrorKind::Oom,
                "out of memory indexing node string cache"
            ));
        }
        self.total_bytes = new_total;
        self.entries.push((node, text));
        Ok(NodeText::Cached(TextId(id)))
    }
}

/// A node's string-value for a comparison: in the cache, a slice of the
/// document, or a text built for this one use because the cache was full.
pub enum NodeText<'d> {
    Cached(TextId),
    Borrowed(&'d [u8]),
    Uncached(Text),
}

impl NodeText<'_> {
    /// The bytes, wherever they are.
    #[inline]
    pub fn bytes<'a>(&'a self, cache: &'a StrCache<'_>) -> &'a [u8] {
        match self {
            NodeText::Cached(id) => cache.text(*id),
            NodeText::Borrowed(s) => s,
            NodeText::Uncached(t) => t.as_slice(),
        }
    }
}
