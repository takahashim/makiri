//! The bounded input cursor: position, line/column, and every scan that reads
//! bytes without deciding what they mean.
//!
//! Both consumers share it - the tree builder in [`super`], which turns what it
//! scans into nodes, and the DTD subset validator in [`super::dtd`], which
//! keeps nothing. Splitting it out is what lets the validator exist without a
//! document: it holds a `&mut Cursor`, so it CANNOT reach the tree, the
//! namespace scope or the node arena, and that is a property of the types
//! rather than a convention.
//!
//! A scan answers an [`InSlice`] - an (offset, length) pair - not a borrow:
//! a borrow of `input` would conflict with the `&mut self` the cursor needs to
//! advance. [`Cursor::slice`] turns one back into bytes, which borrow the
//! INPUT (`'a`), not the cursor.
//!
//! What it deliberately does NOT know is any XML PRODUCTION. "A name", "a quoted
//! literal", "white space" and "everything up to this close" are the vocabulary;
//! ExternalID and the reference grammar of an EntityValue are rules, and they
//! live with the rules ([`super::dtd`]) even though both the DOCTYPE parser and
//! the subset validator need them.

#![forbid(unsafe_code)]

use crate::xml::chars::{decode1, is_name_char, is_name_start, validate_chars};
use crate::xml::qname::split_scanned;
use crate::xml::Status;

/// The parser's result: the detail lives in [`Cursor::status`], which the whole
/// parse shares, so an error only has to say THAT it happened.
pub(super) type R<T = ()> = Result<T, ()>;

/// A slice of the input, as (offset, length). Read it with [`Cursor::slice`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct InSlice {
    pub off: usize,
    pub len: usize,
}

#[inline]
pub(super) fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

#[inline]
pub(super) fn find(h: &[u8], b: u8) -> Option<usize> {
    h.iter().position(|&x| x == b)
}

/// The input, where the scan has reached, and how the parse ended.
///
/// The STATUS is here on purpose, not just the position: it is what lets
/// [`super::dtd::Subset`] report a failure while holding nothing but a cursor,
/// which is the whole reason the validator can exist without a document. The
/// cost is that a couple of READS take `&mut self` ([`Cursor::span`] and
/// [`Cursor::taken_since`], which refuse a length no `u32` span could hold) -
/// worth it, because the alternative is every caller deciding what a too-long
/// slice means.
pub(super) struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    /// The first failure, kept for the whole parse.
    pub status: Status,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(input: &'a [u8]) -> Self {
        Cursor {
            input,
            pos: 0,
            line: 1,
            col: 1,
            status: Status::Ok,
        }
    }

    /* ---- position ---- */

    #[inline]
    pub(super) fn pos(&self) -> usize {
        self.pos
    }
    #[inline]
    pub(super) fn line(&self) -> u32 {
        self.line
    }
    #[inline]
    pub(super) fn col(&self) -> u32 {
        self.col
    }
    #[inline]
    pub(super) fn rest(&self) -> &'a [u8] {
        &self.input[self.pos..]
    }
    #[inline]
    pub(super) fn at_start(&self) -> bool {
        self.pos == 0
    }

    #[inline]
    pub(super) fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }
    #[inline]
    pub(super) fn left(&self) -> usize {
        self.input.len() - self.pos
    }
    #[inline]
    pub(super) fn starts(&self, lit: &[u8]) -> bool {
        self.rest().starts_with(lit)
    }
    /// The bytes an [`InSlice`] names. They borrow the INPUT, so the result
    /// outlives the `&self`.
    #[inline]
    pub(super) fn slice(&self, s: InSlice) -> &'a [u8] {
        &self.input[s.off..s.off + s.len]
    }
    /// The slice from `start` to the cursor, refusing a length no `u32` span
    /// could hold.
    #[inline]
    pub(super) fn taken_since(&mut self, start: usize) -> R<InSlice> {
        self.span(start, self.pos - start)
    }
    #[inline]
    pub(super) fn span(&mut self, off: usize, len: usize) -> R<InSlice> {
        if len > u32::MAX as usize {
            return self.limit();
        }
        Ok(InSlice { off, len })
    }

    #[inline]
    pub(super) fn advance(&mut self) {
        if let Some(c) = self.peek() {
            self.pos += 1;
            if c == b'\n' {
                self.line = self.line.wrapping_add(1);
                self.col = 1;
            } else {
                self.col = self.col.wrapping_add(1);
            }
        }
    }
    /// Advance up to n bytes keeping line/col correct (the span may hold LF).
    pub(super) fn advance_n(&mut self, n: usize) {
        let end = core::cmp::min(self.pos.saturating_add(n), self.input.len());
        for &c in &self.input[self.pos..end] {
            if c == b'\n' {
                self.line = self.line.wrapping_add(1);
                self.col = 1;
            } else {
                self.col = self.col.wrapping_add(1);
            }
        }
        self.pos = end;
    }
    pub(super) fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if is_space(c) {
                self.advance();
            } else {
                break;
            }
        }
    }
    /// Skip white space, answering whether there was any.
    pub(super) fn skip_some_ws(&mut self) -> bool {
        let before = self.pos;
        self.skip_ws();
        self.pos > before
    }

    /* ---- status ---- */

    /// Record `st` as the parse's outcome and give up.
    ///
    /// The FIRST failure wins, for all four kinds: it is the cause, and anything
    /// after it is a consequence of having given up. (`syntax` alone used to be
    /// sticky while `limit`/`unsupported` overwrote, which raised a question
    /// nothing answered - the parse stops at the first failure, so the two
    /// behaved identically and only one can be the rule.)
    #[inline]
    pub(super) fn fail<T>(&mut self, st: Status) -> R<T> {
        if self.status.is_ok() {
            self.status = st;
        }
        Err(())
    }
    #[inline]
    pub(super) fn syntax<T>(&mut self) -> R<T> {
        self.fail(Status::Syntax)
    }
    /// Well-formed, but uses a DTD construct Makiri refuses rather than
    /// silently ignores (see [`super::dtd`]).
    #[inline]
    pub(super) fn unsupported<T>(&mut self) -> R<T> {
        self.fail(Status::Unsupported)
    }
    #[inline]
    pub(super) fn limit<T>(&mut self) -> R<T> {
        self.fail(Status::Limit)
    }
    #[inline]
    pub(super) fn need_space(&mut self) -> R {
        match self.peek() {
            Some(c) if is_space(c) => Ok(()),
            _ => self.syntax(),
        }
    }
    /// White space that must be there, then any more of it.
    #[inline]
    pub(super) fn require_space(&mut self) -> R {
        self.need_space()?;
        self.skip_ws();
        Ok(())
    }

    /* ---- names ---- */

    /// A Name (NameStartChar NameChar*).
    pub(super) fn scan_name(&mut self) -> R<InSlice> {
        let start = self.pos;
        match decode1(self.rest()) {
            Some((cp, bl)) if is_name_start(cp) => self.advance_n(bl),
            _ => return self.syntax(),
        }
        loop {
            match decode1(self.rest()) {
                Some((cp, bl)) if is_name_char(cp) => self.advance_n(bl),
                _ => break,
            }
        }
        self.taken_since(start)
    }

    /// A Name that must also be a QName (Namespaces in XML §3: element and
    /// attribute names, the DOCTYPE's included).
    pub(super) fn scan_qname(&mut self) -> R<InSlice> {
        let s = self.scan_name()?;
        if split_scanned(self.slice(s)).is_none() {
            return self.syntax();
        }
        Ok(s)
    }

    /// Namespaces in XML §7: every other Name - entity, notation, PI target -
    /// is an NCName.
    pub(super) fn scan_ncname(&mut self) -> R<InSlice> {
        let s = self.scan_name()?;
        if self.slice(s).contains(&b':') {
            return self.syntax();
        }
        Ok(s)
    }

    /// Nmtoken (§2.3): one or more NameChars.
    pub(super) fn scan_nmtoken(&mut self) -> R {
        let start = self.pos;
        while let Some((cp, bl)) = decode1(self.rest()) {
            if !is_name_char(cp) {
                break;
            }
            self.advance_n(bl);
        }
        if self.pos == start {
            return self.syntax();
        }
        Ok(())
    }

    /* ---- literals and keywords ---- */

    pub(super) fn eat_keyword(&mut self, kw: &[u8]) -> bool {
        if !self.starts(kw) {
            return false;
        }
        self.advance_n(kw.len());
        true
    }

    /// A quoted literal.
    pub(super) fn parse_quoted(&mut self) -> R<InSlice> {
        let q = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => return self.syntax(),
        };
        self.advance();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == q {
                break;
            }
            self.advance();
        }
        if self.left() == 0 {
            return self.syntax(); /* unterminated / mismatched quote */
        }
        let s = self.taken_since(start)?;
        self.advance(); /* closing quote */
        Ok(s)
    }

    /// A quoted literal whose characters must all be XML Chars.
    pub(super) fn scan_char_literal(&mut self) -> R<InSlice> {
        let s = self.parse_quoted()?;
        if !validate_chars(self.slice(s)) {
            return self.syntax();
        }
        Ok(s)
    }

    /// PubidLiteral (§2.3): a restricted ASCII set.
    pub(super) fn scan_pubid_literal(&mut self) -> R<InSlice> {
        let s = self.parse_quoted()?;
        let ok = self
            .slice(s)
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || b" \r\n-'()+,./:=?;!*#@$_%".contains(&c));
        if !ok {
            return self.syntax();
        }
        Ok(s)
    }

    /// Everything up to the next `close`, which must appear: the ONE scan for
    /// a comment's, a PI's or a CDATA section's body.
    ///
    /// `banned_repeat` is a byte that may not occur doubled before the close -
    /// a comment's `--`, which §2.5 forbids. The cursor is left AT the close,
    /// so the caller decides whether to keep the body and then advances past
    /// `close.len()`; the two callers that keep it and the two that discard it
    /// were four copies of this loop.
    pub(super) fn scan_until_close(
        &mut self,
        close: &[u8],
        banned_repeat: Option<u8>,
    ) -> R<InSlice> {
        let lead = close[0];
        let start = self.pos;
        let mut j = self.pos;
        loop {
            match find(&self.input[j..], lead) {
                Some(at) => j += at,
                None => return self.syntax(), /* unterminated */
            }
            if self.input[j..].starts_with(close) {
                break;
            }
            if banned_repeat == Some(lead) && self.input.get(j + 1) == Some(&lead) {
                return self.syntax(); /* e.g. '--' not part of '-->' */
            }
            j += 1;
        }
        let body = self.span(start, j - start)?;
        if !validate_chars(self.slice(body)) {
            return self.syntax();
        }
        Ok(body)
    }

    /// Consume a body scanned by [`Cursor::scan_until_close`] and its close.
    #[inline]
    pub(super) fn take_close(&mut self, body: InSlice, close: &[u8]) {
        self.advance_n(body.len + close.len());
    }
}
