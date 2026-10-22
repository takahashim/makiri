//! A Document's content as the bridge holds it - the parsed HTML document or
//! the XML arena - and the only ways to reach it: a shared borrow
//! ([`HtmlRead`], [`XmlRead`]) or an exclusive one ([`HtmlWrite`],
//! [`XmlWrite`]), each a guard on the `RefCell` the Document owns its content
//! through.
//!
//! The content used to be a raw pointer that every reader and editor
//! dereferenced under an `unsafe` contract - "the Document is live, and nothing
//! edits it while you hold this" - which each call site had to re-establish. A
//! guard makes the second half a runtime check instead: an edit that begins
//! while any read is held (a Fiber suspended mid-XPath, a reader that ran Ruby)
//! is refused, and so is a read during an edit. The first half - the
//! Document stays alive - is the caller's `&Value`, which the guard's lifetime
//! is tied to; a guard is no GC root.
//!
//! A guard is a `Drop`, so a raise that `longjmp`s past one leaves its borrow
//! counted for good. `rake unsafe:boundaries` therefore treats the guards as
//! it treats an argument's copy: none may be live across a call that can
//! raise, unless that call is made under `bridge::ruby::protected`.

#![forbid(unsafe_code)]

use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes, HtmlNode, RawNode};
use crate::html_dom::post_parse::HtmlParsed;
use crate::xml::model::Document as XmlDoc;

/// What a Document owns: its parsed content, in one of the two
/// representations, borrowed only through the guards below.
pub(in crate::bridge) enum Body {
    Html(RefCell<Box<HtmlParsed>>),
    Xml(RefCell<Box<XmlDoc>>),
}

impl Body {
    /// The bytes the content holds outside Ruby's allocator, or `None` while
    /// an edit holds it (a GC callback's question, answered without waiting).
    pub(in crate::bridge) fn external_bytes(&self) -> Option<usize> {
        match self {
            Body::Html(c) => c.try_borrow().ok().map(|p| p.external_bytes()),
            Body::Xml(c) => c.try_borrow().ok().map(|d| d.memsize()),
        }
    }

    /// The HTML content's chunk count, 0 for XML; `None` while an edit holds
    /// it.
    pub(in crate::bridge) fn arena_chunks(&self) -> Option<usize> {
        match self {
            Body::Html(c) => c.try_borrow().ok().map(|p| p.arena_chunks()),
            Body::Xml(_) => Some(0),
        }
    }
}

/// Why a guard could not be taken: the content is held the other way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::bridge) enum Conflict {
    /// A read was asked for while an edit holds the content - an edit runs
    /// no Ruby, so this is a broken invariant.
    ByEdit,
    /// An edit was asked for while a read holds it - a reader suspended in
    /// Ruby (an XPath handler, a Fiber).
    ByRead,
}

/// An HTML document, borrowed for reading.
pub struct HtmlRead<'a>(Ref<'a, Box<HtmlParsed>>);

impl<'a> HtmlRead<'a> {
    pub(in crate::bridge) fn of(c: &'a RefCell<Box<HtmlParsed>>) -> Result<Self, Conflict> {
        c.try_borrow().map(HtmlRead).map_err(|_| Conflict::ByEdit)
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
pub struct HtmlWrite<'a>(RefMut<'a, Box<HtmlParsed>>);

impl<'a> HtmlWrite<'a> {
    pub(in crate::bridge) fn of(c: &'a RefCell<Box<HtmlParsed>>) -> Result<Self, Conflict> {
        c.try_borrow_mut()
            .map(HtmlWrite)
            .map_err(|_| Conflict::ByRead)
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
pub struct XmlRead<'a>(Ref<'a, Box<XmlDoc>>);

impl<'a> XmlRead<'a> {
    pub(in crate::bridge) fn of(c: &'a RefCell<Box<XmlDoc>>) -> Result<Self, Conflict> {
        c.try_borrow().map(XmlRead).map_err(|_| Conflict::ByEdit)
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
pub struct XmlWrite<'a>(RefMut<'a, Box<XmlDoc>>);

impl<'a> XmlWrite<'a> {
    pub(in crate::bridge) fn of(c: &'a RefCell<Box<XmlDoc>>) -> Result<Self, Conflict> {
        c.try_borrow_mut()
            .map(XmlWrite)
            .map_err(|_| Conflict::ByRead)
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
