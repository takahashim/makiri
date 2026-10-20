//! The parsed HTML document as the bridge holds it: the pointer a Ruby
//! Document owns, and the borrows of it - shared, for an edit, for XPath -
//! that the rest of the bridge takes. An arena handle borrows its document
//! and a node is an id resolved by a checked lookup, so what is the caller's
//! promise here is only that the Document is live and the borrow is the one
//! access the contract says.

#![allow(unsafe_code)]

use core::ptr::NonNull;

use crate::html_dom::html::{HtmlDoc, HtmlDocMut, RawNode};
use crate::html_dom::post_parse::HtmlParsed;

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
    pub unsafe fn edit_new_nodes<'d>(self) -> HtmlDocMut<'d> {
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

/// XPath's view of the document behind `p`, for `'e`. `Err` when the
/// element index a mutation dropped could not be rebuilt.
///
/// The instance reads the index its caller built: built here, into the
/// handle's cell, when it is missing.
///
/// # Safety
/// The Document is live, and nothing edits it, for `'e`.
#[inline]
pub unsafe fn xpath_dom<'e>(
    p: DocPtr,
) -> Result<crate::html_dom::xpath::HtmlDom<'e>, crate::engine_error::ErrorKind> {
    // SAFETY: the caller's contract.
    unsafe { p.parsed() }
        .ensure_dom_index()
        .map_err(|_| crate::engine_error::ErrorKind::Oom)?;
    // SAFETY: the caller's contract.
    Ok(crate::html_dom::xpath::dom(unsafe { p.parsed() }))
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
