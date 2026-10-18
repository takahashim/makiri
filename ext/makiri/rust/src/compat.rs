//! Where the tree builder or the serializer follows the pinned HTML Standard,
//! or the CSS parser CSS Syntax 3 and Selectors 4, and Lexbor does not
//! (`docs/parser_differences.ja.md`, the P, R and C items): each such step
//! asks [`fixed`] whether to take the specification's branch.
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
    /// "In column group" inserts every whitespace character of a text token,
    /// not only the leading run, when the current node is not a `colgroup`.
    P6,
    /// "After after frameset" processes the whitespace of a text token that
    /// also holds other characters, using the "in body" rules.
    P7,
    /// Input that ends inside `[CDATA[` keeps what was read as the bogus
    /// comment's data.
    P8,
    /// A `<!` at the end of the input makes an empty comment, not one holding
    /// the text before it.
    P9,
    /// An attribute written with no value equals one with the value "" when
    /// the active formatting elements are counted.
    P10,
    /// The adoption agency algorithm's bookmark and formatting element
    /// position follow the entries the inner loop removes.
    P11,
    /// A `<textarea>`'s contents are read in "text".
    P12,
    /// An `<input>` whose `type` is "hidden" in any ASCII case leaves
    /// frameset-ok alone.
    P13,
    /// An `<image>` start tag in "in table" is reprocessed as `<img>`, not
    /// lost.
    P14,
    /// A `<sup>` start tag breaks out of foreign content.
    P15,
    /// A fragment parse pops its open elements at the end.
    P16,
    /// A fragment's document takes the context document's quirks mode. Read
    /// from `ContextFacts`, whose default is Lexbor's answer, so [`explains`]
    /// never names it, nor P18 to P20: the harnesses parse with the default.
    P17,
    /// A fragment's MathML `annotation-xml` context is an HTML integration
    /// point when its `encoding` says so.
    P18,
    /// Foreign content adjusts foreign attributes in every namespace, and a
    /// fragment whose context has a namespace the parser does not know (or
    /// none) reads CDATA sections in it.
    P19,
    /// A fragment's form element pointer is the context's nearest form,
    /// itself or an ancestor.
    P20,
    /// A void element's `inner_html` is empty, whatever children the DOM
    /// gave it. The serializer's, as is R2; no parse makes either shape, so
    /// [`explains`] never names them.
    R1,
    /// An element in the HTML, SVG or MathML namespace is serialized by its
    /// local name, without the prefix it was created with.
    R2,
    /// Specificity is Selectors 4's (C23 to C26, one rule): every
    /// pseudo-class counts in B, a prefixed type selector once, a
    /// functional pseudo-class as its most specific argument, and each
    /// component is clamped rather than carried.
    C23,
    /// A token after an `An+B` makes the selector invalid.
    C21,
    /// An empty `:is()` or `:where()` is valid and matches nothing.
    C18,
    /// `:has()` is unforgiving: an invalid item invalidates the selector.
    C19,
    /// `:has()` is not valid inside `:has()`.
    C20,
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

/// Whether the input's last `<!` has no `>` after it.
#[cfg(any(test, feature = "lexbor-reference"))]
fn ends_in_markup_declaration(input: &[u8]) -> bool {
    input
        .windows(2)
        .rposition(|w| w == b"<!")
        .is_some_and(|i| !input[i..].contains(&b'>'))
}

/// Whether the input has an end tag of an element the adoption agency
/// algorithm runs for.
#[cfg(any(test, feature = "lexbor-reference"))]
fn has_formatting_end_tag(input: &[u8]) -> bool {
    const NAMES: [&[u8]; 14] = [
        b"a", b"b", b"big", b"code", b"em", b"font", b"i", b"nobr", b"s", b"small", b"strike",
        b"strong", b"tt", b"u",
    ];
    NAMES.iter().any(|n| {
        input
            .windows(n.len() + 2)
            .any(|w| w[0] == b'<' && w[1] == b'/' && w[2..].eq_ignore_ascii_case(n))
    })
}

/// The fixes that may change what the CSS parser makes of `input`, a
/// selector or a stylesheet (`lexbor::selector_diff`,
/// `lexbor::stylesheet_diff`).
#[cfg(any(test, feature = "lexbor-reference"))]
pub fn css_explains(input: &[u8]) -> Vec<Fix> {
    let mut fixes = Vec::new();
    /* C23: a pseudo-class or pseudo-element, a namespace prefix, or enough
     * simple selectors to fill a component (511). */
    if input.contains(&b':') || input.contains(&b'|') || input.len() >= 1000 {
        fixes.push(Fix::C23);
    }
    /* C18: an `:is()` or `:where()` left empty. */
    if has(input, b"is(") || has(input, b"where(") {
        fixes.push(Fix::C18);
    }
    /* C19, C20: a `:has()`. */
    if has(input, b"has(") {
        fixes.push(Fix::C19);
        fixes.push(Fix::C20);
    }
    /* C21: a token after an `An+B`. */
    if has(input, b"nth-") {
        fixes.push(Fix::C21);
    }
    fixes
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
    /* P6: text after a non-whitespace character in "in column group" with
     * no colgroup as the current node: a template's, or a colgroup context. */
    if colgroup || (has(input, b"col") && has(input, b"template")) {
        fixes.push(Fix::P6);
    }
    /* P7: text after `</html>` in a frameset document. */
    if has(input, b"frameset") {
        fixes.push(Fix::P7);
    }
    /* P8, P9: the input ends inside a markup declaration. */
    if ends_in_markup_declaration(input) {
        fixes.push(Fix::P8);
        fixes.push(Fix::P9);
    }
    /* P10: an attribute whose value is the empty string, which can only be
     * written quoted. */
    if has(input, b"\"\"") || has(input, b"''") {
        fixes.push(Fix::P10);
    }
    /* P11: the inner loop removes an entry only from its fourth step, so
     * the furthest block lies at least four elements below the formatting
     * element: an end tag closing a formatting element across that many
     * open elements, one of them special. Any input with an end tag of a
     * formatting element, or an `<a>` or `<nobr>` start tag (which run the
     * algorithm too), is admitted. */
    if has_formatting_end_tag(input) || has(input, b"<a") || has(input, b"<nobr") {
        fixes.push(Fix::P11);
    }
    /* P12: a `<textarea>`'s contents. */
    if has(input, b"textarea") {
        fixes.push(Fix::P12);
    }
    /* P13: an `<input>` before a `<frameset>`. */
    if has(input, b"frameset") && has(input, b"hidden") {
        fixes.push(Fix::P13);
    }
    /* P14: an `<image>` start tag reaching "in table"'s anything else. */
    if has(input, b"image") {
        fixes.push(Fix::P14);
    }
    /* P15: a `<sup>` in foreign content. */
    if has(input, b"sup")
        && (has(input, b"svg") || has(input, b"math") || matches!(context, Some("svg" | "math")))
    {
        fixes.push(Fix::P15);
    }
    /* P16: the only pop steps are an `<option>`'s, which copy it into a
     * `<selectedcontent>`. */
    if context.is_some() && has(input, b"selectedcontent") {
        fixes.push(Fix::P16);
    }
    fixes
}
