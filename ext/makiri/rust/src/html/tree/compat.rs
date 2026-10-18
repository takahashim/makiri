//! Where the tree builder follows the pinned HTML Standard and Lexbor does not
//! (`docs/parser_differences.ja.md`, the P items): each such step asks
//! [`fixed`] whether to take the Standard's branch.
//!
//! A build that ships always does. The tests and the reference builds can run
//! the port as Lexbor ran ([`as_lexbor`]), which is how the differential
//! harnesses (`lexbor::tree_diff`, the `html_diff` fuzz target) keep holding
//! the port to Lexbor's tree exactly: they compare the Lexbor-compatible run
//! against Lexbor, and then the Standard's run against the compatible one,
//! where a difference must be one an entry of [`explains`] admits - "this
//! shape of input, and only this shape, may change" (docs/rewrite_parser.ja.md,
//! 変更の二つの種類). A fix that also changed something else is caught there.

/// One departure from Lexbor, named by its entry in the differences table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Inserting `<body>` implicitly resets frameset-ok (whatwg/html#12908).
    P1,
    /// "Parsing template contents" decides the form element pointer.
    P2,
    /// A U+FFFD in the input, in foreign content, sets frameset-ok to "not ok".
    P3,
    /// "In table" takes character tokens to "in table text" when the current
    /// node is a `template` too.
    P4,
    /// "In column group" ignores a DOCTYPE token.
    P5,
    /// An element carries its start tag's offset when it was created for
    /// that tag's token, and no other (`Node#line`); not a tree change, so
    /// [`explains`] never names it: the harnesses compare the trees without
    /// offsets and check the offsets on their own.
    L1,
}

#[cfg(any(test, feature = "lexbor-reference"))]
thread_local! {
    static AS_LEXBOR: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Whether the parse takes the Standard's branch where `fix` departs from
/// Lexbor: always, except inside [`as_lexbor`].
#[inline]
pub(crate) fn fixed(fix: Fix) -> bool {
    let _ = fix;
    #[cfg(any(test, feature = "lexbor-reference"))]
    if AS_LEXBOR.with(core::cell::Cell::get) {
        return false;
    }
    true
}

/// Run `f` with every fix off: the port parses as Lexbor did. For the
/// differential harnesses only.
#[cfg(any(test, feature = "lexbor-reference"))]
pub fn as_lexbor<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            AS_LEXBOR.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(AS_LEXBOR.with(|c| c.replace(true)));
    f()
}

/// Whether the input holds `word`, ignoring ASCII case.
#[cfg(any(test, feature = "lexbor-reference"))]
fn has(input: &[u8], word: &[u8]) -> bool {
    input
        .windows(word.len())
        .any(|w| w.eq_ignore_ascii_case(word))
}

/// The fixes that may change the tree of `input` (parsed as a document, or
/// as a fragment in an HTML `context` element named in lower case). An input
/// no fix explains must build the same tree with the fixes on and off.
#[cfg(any(test, feature = "lexbor-reference"))]
pub fn explains(input: &[u8], context: Option<&str>) -> Vec<Fix> {
    let mut fixes = Vec::new();
    /* P1: frameset-ok decides only whether a `<frameset>` start tag is taken,
     * and before the implicit `<body>` only a `<template>` in the head sets
     * it to "not ok". */
    if has(input, b"frameset") && has(input, b"template") {
        fixes.push(Fix::P1);
    }
    /* P2: a form start tag inside template contents - a template open, or a
     * fragment parsed in one. */
    if has(input, b"form") && (has(input, b"template") || context == Some("template")) {
        fixes.push(Fix::P2);
    }
    /* P3: a U+FFFD - written, from a character reference, or where the
     * input was not UTF-8 - in foreign content, before a `<frameset>`. */
    if has(input, b"frameset")
        && (has(input, b"svg") || has(input, b"math"))
        && (has(input, "\u{FFFD}".as_bytes())
            || input.contains(&b'&')
            || core::str::from_utf8(input).is_err())
    {
        fixes.push(Fix::P3);
    }
    /* P4: text where the current node is a template in "in table" - a
     * template's table contents. */
    if has(input, b"template") {
        fixes.push(Fix::P4);
    }
    /* P5: a DOCTYPE in "in column group". */
    let colgroup = context == Some("colgroup");
    if has(input, b"doctype") && (has(input, b"col") || colgroup) {
        fixes.push(Fix::P5);
    }
    fixes
}
