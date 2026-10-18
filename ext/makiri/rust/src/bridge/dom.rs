//! What the bridge does differently for the two HTML DOMs `crate::html_dom`
//! chooses between - the Lexbor adapter and the parser port's arena - kept in
//! this one module so the rest of the bridge reads the same for both.
//!
//! Each step is the same contract on both sides; only how it is met differs.
//! A Lexbor handle is a pointer, so a node read from a word, or a document lent
//! for an edit, is the caller's promise. An arena handle borrows its document,
//! and a node is an id resolved by a checked lookup, so the same steps are
//! ordinary borrows of the parsed handle the Ruby Document owns.

#![allow(unsafe_code)]

use core::ptr::NonNull;

use crate::html_dom::html::{ForeignNode, HtmlDoc, HtmlDocMut, HtmlNode, HtmlNodeKey, RawNode};
use crate::html_dom::post_parse::HtmlParsed;
use crate::token::{Kind, Token};

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
        #[cfg(not(feature = "arena-dom"))]
        // SAFETY: the caller's contract, which is `assume_exclusive`'s.
        let d = unsafe { HtmlDocMut::assume_exclusive(self.as_doc()) };
        #[cfg(feature = "arena-dom")]
        // SAFETY: the caller's contract: the one access for `'d`.
        let d = unsafe { (*self.0.as_ptr()).edit_keeping_indexes() };
        d
    }

    /// The document, lent for an edit of its tree, attributes or data. The
    /// caller drops the indexes first (`wrapper::invalidate_indexes`).
    ///
    /// # Safety
    /// As [`edit_new_nodes`](Self::edit_new_nodes).
    #[inline]
    pub unsafe fn edit<'d>(self) -> HtmlDocMut<'d> {
        #[cfg(not(feature = "arena-dom"))]
        // SAFETY: as `edit_new_nodes`.
        let d = unsafe { HtmlDocMut::assume_exclusive(self.as_doc()) };
        #[cfg(feature = "arena-dom")]
        // SAFETY: as `edit_new_nodes`; `edit` drops the indexes itself too.
        let d = unsafe { (*self.0.as_ptr()).edit() };
        d
    }
}

/// The node `raw` names in `doc`.
///
/// # Safety
/// `raw` is a live node of `doc` - for Lexbor, a node pointer the caller
/// vouches for; the arena checks the id itself.
#[inline]
pub unsafe fn node_in<'d>(doc: HtmlDoc<'d>, raw: RawNode) -> HtmlNode<'d> {
    #[cfg(not(feature = "arena-dom"))]
    // SAFETY: the caller's contract.
    let n = unsafe { doc.node(raw) };
    #[cfg(feature = "arena-dom")]
    let n = doc.node(raw);
    n
}

/// A node as the word the bridge stores (`NodeWord`) and the child memo
/// records: Lexbor's node pointer, or the arena's node id.
#[inline]
pub fn node_word(n: RawNode) -> usize {
    #[cfg(not(feature = "arena-dom"))]
    let w = n.as_ptr() as usize;
    #[cfg(feature = "arena-dom")]
    let w = n.word();
    w
}

/// The node a [`node_word`] names; `None` for 0.
///
/// # Safety
/// The word was made by [`node_word`] from a node of a document that is
/// still alive.
#[inline]
pub unsafe fn node_of_word(w: usize) -> Option<RawNode> {
    #[cfg(not(feature = "arena-dom"))]
    let n = RawNode::from_ptr(w as *mut core::ffi::c_void);
    #[cfg(feature = "arena-dom")]
    let n = RawNode::from_word(w);
    n
}

/// The XPath token of the HTML node a [`node_word`] names.
///
/// # Safety
/// As [`node_of_word`].
#[inline]
pub unsafe fn html_token(w: usize) -> Token {
    #[cfg(not(feature = "arena-dom"))]
    // SAFETY: the caller's contract, which is `Token::html`'s.
    let t = unsafe { Token::html(w as *mut core::ffi::c_void) };
    #[cfg(feature = "arena-dom")]
    let t = Token::arena(w);
    t
}

/// The token kind an HTML document's nodes carry.
#[cfg(not(feature = "arena-dom"))]
pub const HTML_TOKEN_KIND: Kind = Kind::Html;
/// The token kind an HTML document's nodes carry.
#[cfg(feature = "arena-dom")]
pub const HTML_TOKEN_KIND: Kind = Kind::Arena;

/// XPath's view of the document behind `p`, for `'e`. `Err` when the
/// element index a mutation dropped could not be rebuilt.
///
/// The arena's instance reads the index its caller built, where Lexbor's
/// rebuilds it in `Dom::prepare`: built here, with the one `&mut` of the
/// handle an evaluate takes - only when the index is missing, which no
/// evaluation already running can see (one would have built it, and no
/// mutation runs while one does), so it never overlaps a shared borrow.
///
/// # Safety
/// The Document is live, and nothing edits it, for `'e`.
#[inline]
pub unsafe fn xpath_dom<'e>(
    p: DocPtr,
) -> Result<crate::html_dom::xpath::HtmlDom<'e>, crate::engine_error::ErrorKind> {
    #[cfg(not(feature = "arena-dom"))]
    // SAFETY: the caller's contract, which is `dom`'s.
    let d = unsafe { crate::html_dom::xpath::dom(p.as_ptr()) };
    #[cfg(feature = "arena-dom")]
    let d = {
        // SAFETY: see above - no shared borrow of the handle is live while
        // the index is missing.
        if unsafe { p.parsed() }.dom_index().is_none() {
            unsafe { &mut *p.as_ptr().as_ptr() }
                .ensure_dom_index()
                .map_err(|_| crate::engine_error::ErrorKind::Oom)?;
        }
        // SAFETY: the caller's contract.
        crate::html_dom::xpath::dom(unsafe { p.parsed() })
    };
    Ok(d)
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

/// The key for `node` under `parsed` - see `HtmlParsed::mint_key`.
///
/// # Safety
/// `node` is a live node, and `parsed`'s Document stays alive at its address
/// for as long as the key is used (Lexbor's key holds the node pointer; the
/// arena checks the id itself).
#[inline]
pub unsafe fn mint_key(parsed: &HtmlParsed, node: RawNode) -> Result<HtmlNodeKey, ForeignNode> {
    #[cfg(not(feature = "arena-dom"))]
    // SAFETY: the caller's contract, which is `mint_key`'s.
    let k = unsafe { parsed.mint_key(node) };
    #[cfg(feature = "arena-dom")]
    let k = parsed.mint_key(node);
    k
}

/// The 1-based source line of `node`, a node of `parsed`'s document.
///
/// # Safety
/// `node` is a live node of that document.
#[inline]
pub unsafe fn node_line(parsed: &HtmlParsed, node: RawNode) -> Option<usize> {
    #[cfg(not(feature = "arena-dom"))]
    // SAFETY: the caller's contract.
    let l = unsafe { parsed.node_line(node) };
    #[cfg(feature = "arena-dom")]
    let l = parsed.node_line(node);
    l
}
