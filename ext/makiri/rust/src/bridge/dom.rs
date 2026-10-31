//! A Document's content as the bridge holds it - the parsed HTML document or
//! the XML arena - and the only ways to reach it: a shared borrow
//! ([`HtmlRead`], [`XmlRead`]) or an exclusive one ([`HtmlWrite`],
//! [`XmlWrite`]), each a guard on the [`DocCell`] the Document owns its
//! content through. The cell and its guards are `crate::doc_cell`'s, which is
//! Ruby-free so what they promise is tested there; this module names the two
//! representations' guards and what each lends.
//!
//! The content used to be a raw pointer that every reader and editor
//! dereferenced under an `unsafe` contract - "the Document is live, and nothing
//! edits it while you hold this" - which each call site had to re-establish. A
//! guard makes the second half a runtime check instead: an edit that begins
//! while any read is held is refused, and so is a read during an edit. The
//! first half - the Document stays alive - is the caller's `DocHandle`, which
//! holds the Document and which the guard borrows.
//!
//! A guard is a `Drop`, so a raise that `longjmp`s past one would leave its
//! borrow counted for good. None does: every raising call returns `Err`
//! (`nupur::raw`), and `rake oom`'s raise sweep checks that no document is
//! left borrowed.

#![forbid(unsafe_code)]

use core::ops::{Deref, DerefMut};

pub(in crate::bridge) use crate::doc_cell::Conflict;
use crate::doc_cell::{DocCell, Read, Write};
use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes, HtmlNode, RawNode};
use crate::html_dom::post_parse::HtmlParsed;
use crate::xml::model::Document as XmlDoc;

/// What a Document owns: its parsed content, in one of the two
/// representations, borrowed only through the guards below.
pub(in crate::bridge) enum Body {
    Html(DocCell<HtmlParsed>),
    Xml(DocCell<XmlDoc>),
}

impl Body {
    /// The bytes the content holds outside Ruby's allocator, or `None` while
    /// an edit holds it (a GC callback's question, answered without waiting).
    pub(in crate::bridge) fn external_bytes(&self) -> Option<usize> {
        match self {
            Body::Html(c) => c.peek(HtmlParsed::external_bytes),
            Body::Xml(c) => c.peek(XmlDoc::memsize),
        }
    }

    /// The HTML content's chunk count, 0 for XML; `None` while an edit holds
    /// it.
    pub(in crate::bridge) fn arena_chunks(&self) -> Option<usize> {
        match self {
            Body::Html(c) => c.peek(HtmlParsed::arena_chunks),
            Body::Xml(_) => Some(0),
        }
    }
}

/// An HTML document, borrowed for reading.
pub struct HtmlRead<'a>(Read<'a, HtmlParsed>);

impl<'a> HtmlRead<'a> {
    pub(in crate::bridge) fn of(c: &'a DocCell<HtmlParsed>) -> Result<Self, Conflict> {
        c.read().map(HtmlRead)
    }

    /// The parse handle, with its indexes.
    #[inline]
    pub fn parsed(&self) -> &HtmlParsed {
        &self.0
    }

    /// The document.
    #[inline]
    pub fn doc(&self) -> HtmlDoc<'_> {
        self.0.doc()
    }

    /// The node `raw` names, unchecked - for a reference the caller already
    /// knows belongs to this document (a receiver's own).
    #[inline]
    pub fn node(&self, raw: RawNode) -> HtmlNode<'_> {
        self.doc().node(raw)
    }
}

/// An HTML document, borrowed for an edit.
pub struct HtmlWrite<'a>(Write<'a, HtmlParsed>);

impl<'a> HtmlWrite<'a> {
    pub(in crate::bridge) fn of(c: &'a DocCell<HtmlParsed>) -> Result<Self, Conflict> {
        c.write().map(HtmlWrite)
    }

    /// The parse handle, read through the edit's own borrow - for what an
    /// edit reads before it changes anything (an adopted node's source).
    #[inline]
    pub fn parsed(&self) -> &HtmlParsed {
        &self.0
    }

    /// The document, for an edit of its tree, attributes or data, with its
    /// indexes dropped (`HtmlParsed::edit`).
    #[inline]
    pub fn edit(&mut self) -> HtmlDocMut<'_> {
        self.0.edit()
    }

    /// The document, for an edit that only makes detached nodes - the
    /// factories and copies - so the indexes stay.
    #[inline]
    pub fn new_nodes(&mut self) -> HtmlNewNodes<'_> {
        self.0.edit_keeping_indexes()
    }
}

/// An XML arena, borrowed for reading.
pub struct XmlRead<'a>(Read<'a, XmlDoc>);

impl<'a> XmlRead<'a> {
    pub(in crate::bridge) fn of(c: &'a DocCell<XmlDoc>) -> Result<Self, Conflict> {
        c.read().map(XmlRead)
    }
}

impl Deref for XmlRead<'_> {
    type Target = XmlDoc;
    #[inline]
    fn deref(&self) -> &XmlDoc {
        &self.0
    }
}

/// An XML arena, borrowed for an edit.
pub struct XmlWrite<'a>(Write<'a, XmlDoc>);

impl<'a> XmlWrite<'a> {
    pub(in crate::bridge) fn of(c: &'a DocCell<XmlDoc>) -> Result<Self, Conflict> {
        c.write().map(XmlWrite)
    }
}

impl Deref for XmlWrite<'_> {
    type Target = XmlDoc;
    #[inline]
    fn deref(&self) -> &XmlDoc {
        &self.0
    }
}

impl DerefMut for XmlWrite<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut XmlDoc {
        &mut self.0
    }
}
