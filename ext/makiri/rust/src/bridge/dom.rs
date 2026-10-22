//! A Document's content as the bridge holds it - the parsed HTML document
//! ([`DocPtr`]) or the XML arena ([`XmlDocPtr`]): the pointer a Ruby
//! Document owns, and the borrows of it - shared, for an edit, for XPath -
//! that the rest of the bridge takes. Each borrow is one named `unsafe`
//! method, so its contract is stated once. An arena handle borrows its document
//! and a node is an id resolved by a checked lookup, so what is the caller's
//! promise here is only that the Document is live and the borrow is the one
//! access the contract says.

#![allow(unsafe_code)]

use core::ptr::NonNull;

use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes, RawNode};
use crate::html_dom::post_parse::HtmlParsed;
use crate::xml::model::Document as XmlDoc;

/// The parsed document a Ruby HTML Document owns, as the bridge passes it:
/// the pointer `DocumentShell::install_html` leaked, live for as long as the
/// Document.
#[derive(Clone, Copy)]
pub struct DocPtr(NonNull<HtmlParsed>);

impl DocPtr {
    /// The handle behind `p`.
    #[inline]
    pub fn new(p: NonNull<HtmlParsed>) -> Self {
        DocPtr(p)
    }

    /// The pointer.
    #[inline]
    pub fn as_ptr(self) -> NonNull<HtmlParsed> {
        self.0
    }

    /// The parsed handle, shared, for `'d`.
    ///
    /// # Safety
    /// The Document is live for `'d`, and nothing edits it while `'d` lasts.
    #[inline]
    pub unsafe fn parsed<'d>(self) -> &'d HtmlParsed {
        // SAFETY: the caller's contract.
        unsafe { &*self.0.as_ptr() }
    }

    /// The document, for reading during `'d`.
    ///
    /// # Safety
    /// As [`parsed`](Self::parsed).
    #[inline]
    pub unsafe fn as_doc<'d>(self) -> HtmlDoc<'d> {
        // SAFETY: the caller's contract.
        unsafe { self.parsed() }.doc()
    }

    /// The document, lent for an edit that only makes detached nodes - the
    /// factories and copies - so the indexes stay.
    ///
    /// # Safety
    /// The document is cleared for editing (no evaluation reads it), this is
    /// the only access to it for `'d` - no handle of it is used while this
    /// one lives - and no Ruby runs meanwhile.
    #[inline]
    pub unsafe fn edit_new_nodes<'d>(self) -> HtmlNewNodes<'d> {
        // SAFETY: the caller's contract: the one access for `'d`.
        unsafe { (*self.0.as_ptr()).edit_keeping_indexes() }
    }

    /// The document, lent for an edit of its tree, attributes or data, with
    /// its indexes dropped (`HtmlParsed::edit`).
    ///
    /// # Safety
    /// As [`edit_new_nodes`](Self::edit_new_nodes).
    #[inline]
    pub unsafe fn edit<'d>(self) -> HtmlDocMut<'d> {
        // SAFETY: as `edit_new_nodes`.
        unsafe { (*self.0.as_ptr()).edit() }
    }
}

/// The XML arena a Ruby XML Document owns, as the bridge passes it: the
/// pointer `DocumentShell::install_xml` leaked, live for as long as the
/// Document. Equality is identity: two are equal when they are one arena.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct XmlDocPtr(NonNull<XmlDoc>);

impl XmlDocPtr {
    /// The handle behind `p`.
    #[inline]
    pub fn new(p: NonNull<XmlDoc>) -> Self {
        XmlDocPtr(p)
    }

    /// The arena, shared, for `'d`.
    ///
    /// # Safety
    /// The Document is live for `'d`, and nothing edits it while `'d` lasts.
    #[inline]
    pub unsafe fn as_ref<'d>(self) -> &'d XmlDoc {
        // SAFETY: the caller's contract.
        unsafe { &*self.0.as_ptr() }
    }

    /// The arena, lent for an edit.
    ///
    /// # Safety
    /// The document is cleared for editing (no evaluation reads it), this is
    /// the only access to it for `'d` - no other borrow of it is used while
    /// this one lives - and no Ruby runs meanwhile.
    #[inline]
    pub unsafe fn edit<'d>(self) -> &'d mut XmlDoc {
        // SAFETY: the caller's contract: the one access for `'d`.
        unsafe { &mut *self.0.as_ptr() }
    }
}

/// XPath's view of the document behind `p`, for `'e`. The element index is
/// built by the first `//tag` that wants it (`HtmlDom::name_bucket`).
///
/// # Safety
/// The Document is live, and nothing edits it, for `'e`.
#[inline]
pub unsafe fn xpath_dom<'e>(p: DocPtr) -> crate::html_dom::xpath::HtmlDom<'e> {
    // SAFETY: the caller's contract.
    crate::html_dom::xpath::dom(unsafe { p.parsed() })
}

/// The document node of `p`'s document.
///
/// # Safety
/// As [`DocPtr::parsed`].
#[inline]
pub unsafe fn document_node(p: DocPtr) -> RawNode {
    // SAFETY: the caller's contract.
    RawNode::from(unsafe { p.as_doc() }.as_node())
}
