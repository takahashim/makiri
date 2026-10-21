//! A parsed arena document, as a Ruby Document holds it: the [`Document`], its
//! line table, and the indexes built over it on demand - the counterpart of the
//! Lexbor adapter's `post_parse::HtmlParsed`, with the same methods.
//!
//! The indexes themselves are `index.rs`'s. They name nodes by id and text
//! by its place in the byte store, so nothing here borrows across a change:
//! an edit takes `&mut` of the whole handle ([`HtmlParsed::edit`]), which
//! drops the indexes first.

use core::cell::OnceCell;
use core::ptr::NonNull;

use super::edit::{HtmlDocMut, HtmlNewNodes};
use super::handle::{HtmlDoc, HtmlNode, RawNode, TagId};
use super::index::{DomIndex, TextIndex, TextRun};
use super::{Document, DomOom, NodeId};
use crate::source_lines::Lines;

/// The identity of a live parsed document: the address of its
/// [`HtmlParsed`], stable for as long as the Ruby Document that owns it.
/// Compare-only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlDocIdentity(NonNull<HtmlParsed>);

/// A node that is not a node of the document it was keyed or resolved
/// against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ForeignNode;

/// A long-lived node handle: the node and the identity of its document.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlNodeKey {
    owner: HtmlDocIdentity,
    node: RawNode,
}

impl HtmlNodeKey {
    /// The node, for the bridge's node identity and transport.
    #[inline]
    pub fn raw_node(self) -> RawNode {
        self.node
    }
}

/// A parsed HTML document, and the indexes built over it on demand.
///
/// The indexes are cells a READER fills (`&self`): a query builds the one it
/// needs while other readers - an XPath walk that called a handler, a node
/// wrapper being made - hold the document shared, so building one must not
/// take it `&mut`. Only an edit drops them, and an edit holds the document
/// alone (`edit`, behind the mutation gate).
pub struct HtmlParsed {
    doc: Document,
    dom_index: OnceCell<DomIndex>,
    lines: Option<Lines>,
    /// `None` inside: the document has no root element to index under.
    text_index: OnceCell<Option<TextIndex>>,
}

impl HtmlParsed {
    /// A parsed document, with its line table when there is one.
    pub fn new(doc: Document, lines: Option<Lines>) -> Self {
        HtmlParsed {
            doc,
            dom_index: OnceCell::new(),
            lines,
            text_index: OnceCell::new(),
        }
    }

    /// The document, for reading.
    #[inline]
    pub fn doc(&self) -> HtmlDoc<'_> {
        HtmlDoc::of(&self.doc)
    }

    /// The document, lent for an edit. The indexes are dropped first, so no
    /// index outlives the tree it describes.
    #[inline]
    pub fn edit(&mut self) -> HtmlDocMut<'_> {
        self.invalidate_indexes();
        /* No span is held outside the arena now (`compact.rs`). A store that
         * cannot be allocated leaves everything as it was. */
        if self.doc.compaction_due() {
            let _ = self.doc.compact();
        }
        HtmlDocMut::new(&mut self.doc)
    }

    /// The document, lent to make DETACHED nodes - factories and copies, and
    /// a detached fragment's subtree - with the indexes kept: no index names
    /// a node no tree holds, and nothing an index stores moves when the
    /// arena grows (ids and offsets). Anything that changes a node in the
    /// document's tree takes [`edit`](Self::edit), which drops them.
    #[inline]
    pub fn edit_keeping_indexes(&mut self) -> HtmlNewNodes<'_> {
        HtmlNewNodes::new(&mut self.doc)
    }

    /// The arena itself.
    #[inline]
    pub fn arena(&self) -> &Document {
        &self.doc
    }

    /// Build the tag -> elements index if it is not built. `Err` when the
    /// build cannot allocate, which caches nothing.
    pub fn ensure_dom_index(&self) -> Result<(), DomOom> {
        if self.dom_index.get().is_none() {
            let index = DomIndex::build(self.doc()).ok_or(DomOom)?;
            /* Unset just now, and nothing between could set it: the build
             * runs no Ruby. */
            let _ = self.dom_index.set(index);
        }
        Ok(())
    }

    /// The tag -> elements index, once built.
    pub fn dom_index(&self) -> Option<&DomIndex> {
        self.dom_index.get()
    }

    /// The elements with tag `tag`, in document order, by id; `None` until
    /// the index is built, and for a tag it does not bucket.
    pub fn tag_bucket(&self, tag: TagId) -> Option<&[NodeId]> {
        self.dom_index.get()?.tag_bucket(tag)
    }

    /// Build the node -> text index if it is not built and can be.
    pub fn ensure_text_index(&self) -> Result<(), DomOom> {
        if self.text_index.get().is_none() {
            let built = match self.doc().as_node().document_root() {
                Some(root) => TextIndex::build(root)?,
                None => None,
            };
            /* As `ensure_dom_index`: unset just now, and the build runs no
             * Ruby. A failed build sets nothing, so a later call retries. */
            let _ = self.text_index.set(built);
        }
        Ok(())
    }

    /// The run of text slices `node`'s subtree owns, or `None`: walk
    /// instead.
    pub fn text_slices(&self, node: RawNode) -> Option<TextRun<'_>> {
        self.text_index.get()?.as_ref()?.slices_of(&self.doc, node)
    }

    /// Drop the indexes so the next query rebuilds them.
    pub fn invalidate_indexes(&mut self) {
        self.dom_index.take();
        self.text_index.take();
    }

    /// The bytes this document holds outside Ruby's allocator.
    pub fn external_bytes(&self) -> usize {
        self.doc.owned_bytes()
    }

    /// An O(1) measure that grows with the document, for deciding when to
    /// measure [`external_bytes`](Self::external_bytes) again: the node count
    /// plus the byte store and the name and namespace tables in 4 KiB pages.
    pub fn arena_chunks(&self) -> usize {
        self.doc.node_count() / 64 + (self.doc.stored_bytes() + self.doc.table_growth()) / 4096
    }

    /// The 1-based source line for `node`, or `None` when unknown.
    pub fn node_line(&self, node: RawNode) -> Option<usize> {
        let lines = self.lines.as_ref()?;
        let offset = self.doc().checked(node)?.source_offset()?;
        Some(lines.lookup(offset))
    }

    /// The identity of this document, for a node key.
    #[inline]
    pub fn identity(&self) -> HtmlDocIdentity {
        HtmlDocIdentity(NonNull::from(self))
    }

    /// The key for `node`, which must be a node of this document.
    pub fn mint_key(&self, node: RawNode) -> Result<HtmlNodeKey, ForeignNode> {
        if self.doc().checked(node).is_none() {
            return Err(ForeignNode);
        }
        Ok(HtmlNodeKey {
            owner: self.identity(),
            node,
        })
    }

    /// The node `key` names, when it belongs to this document.
    pub fn resolve(&self, key: HtmlNodeKey) -> Result<HtmlNode<'_>, ForeignNode> {
        if key.owner != self.identity() {
            return Err(ForeignNode);
        }
        self.doc().checked(key.node).ok_or(ForeignNode)
    }
}
