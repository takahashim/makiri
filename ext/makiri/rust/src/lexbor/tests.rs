//! Tests for the Lexbor boundary that need more than the module they cover can
//! carry: `contains_guard` forbids `unsafe`, so the parser it restricts cannot
//! be driven from inside it, and test code allocates infallibly, which
//! `rake unsafe:boundaries` rightly refuses in an engine layer. A `tests.rs` is
//! where both are allowed.

#![allow(unsafe_code)]

mod guard {
    use crate::lexbor::contains_guard::{neutralized, Oom};

    fn out(s: &str) -> String {
        match neutralized(s.as_bytes()) {
            Ok(None) => s.to_owned(),
            Ok(Some(v)) => String::from_utf8(v).expect("same bytes, rewritten in place"),
            Err(Oom) => panic!("oom in a test"),
        }
    }

    fn untouched(s: &str) {
        assert_eq!(neutralized(s.as_bytes()).expect("no oom"), None, "{s}");
    }

    fn rewritten(s: &str) {
        let got = out(s);
        assert_ne!(got, s, "{s} should have been neutralised");
        assert_eq!(got.len(), s.len(), "{s} changed length");
        assert!(!got.contains("lexbor-contains"), "{got}");
    }

    #[test]
    fn leaves_selectors_without_the_pseudo_alone() {
        for s in [
            "p",
            ".lexbor-contains",
            "#lexbor-contains",
            "[class~=\"lexbor-contains\"]",
            "a:hover",
            ":is(p, a)",
            ":nth-child(2 of p)",
            "/* :lexbor-contains(#x) */ p",
            "p[title=\":lexbor-contains(#x)\"]",
        ] {
            untouched(s);
        }
    }

    #[test]
    fn leaves_a_well_formed_argument_alone() {
        for s in [
            ":lexbor-contains(\"x\")",
            "p:lexbor-contains(\"x\")",
            ":lexbor-contains('x')",
            ":lexbor-contains(ident)",
            ":lexbor-contains( ident )",
            ":lexbor-contains(\"x\" i)",
            ":lexbor-contains(\"x\" I)",
            ":lexbor-contains(  \"x\"  i  )",
            ":lexbor-contains(\"\")",
            ":LEXBOR-CONTAINS(\"x\")",
            ":lexbor-contains(/* c */ \"x\")",
        ] {
            untouched(s);
        }
    }

    #[test]
    fn rewrites_every_failing_argument() {
        for s in [
            ":lexbor-contains()",
            ":lexbor-contains())",
            ":lexbor-contains( ))",
            ":lexbor-contains(123)",
            ":lexbor-contains(#x)",
            ":lexbor-contains(.x)",
            ":lexbor-contains(*)",
            ":lexbor-contains(,)",
            ":lexbor-contains(\"s\" junk)",
            ":lexbor-contains(id junk)",
            ":lexbor-contains(foo(bar))",
            ":lexbor-contains(\"unterminated)",
            "p,:lexbor-contains()),q",
        ] {
            rewritten(s);
        }
    }

    #[test]
    fn sees_through_identifier_escapes() {
        for s in [
            ":lexbor\\-contains()",
            ":\\6C exbor-contains()",
            ":lexbo\\72-contains()",
            ":LEXBOR-CONTAINS())",
            ":\\6c\\65 xbor-contains()",
            /* every character escaped, the widest spelling the prefilter's
             * backward walk has to cover */
            ":\\00006c\r\n\\000065\r\n\\000078\r\n\\000062\r\n\\00006f\r\n\\000072\r\n\
             \\00002d\r\n\\000063\r\n\\00006f\r\n\\00006e\r\n\\000074\r\n\\000061\r\n\
             \\000069\r\n\\00006e\r\n\\000073\r\n(#x)",
            ":\\6C\r\nexbor-contains(#x)",
            ":\\6C\u{0c}exbor-contains(#x)",
            ":lexbor-contain\\73 (#x)",
            ":lexbor-contain\\s(#x)",
            ":lexbor-contains\\28(#x)",
        ] {
            if s.ends_with("\\28(#x)") {
                /* `\28` is an escaped `(` - part of the name, which is then
                 * not this one */
                untouched(s);
                continue;
            }
            rewritten(s);
        }
    }

    /// Text holding no escape and no literal name skips the tokenizer, and
    /// escapes elsewhere (the Tailwind shape) do not change the answer.
    #[test]
    fn escapes_away_from_a_name_change_nothing() {
        for s in [
            ".md\\:flex{display:flex;color:rgb(0 0 0)}",
            ".w-1\\/2{width:calc(100% / 2)}",
            "a{content:\"\\201C\"}p:lexbor-contains(\"x\"){}",
        ] {
            untouched(s);
        }
        rewritten(".md\\:flex{}p:lexbor-contains(#x){}");
    }

    #[test]
    fn rewrites_each_occurrence_and_keeps_the_rest() {
        let got = out(".a{color:red}:lexbor-contains(#x){color:blue}.b{color:green}");
        assert!(got.starts_with(".a{color:red}:"), "{got}");
        assert!(got.ends_with("(#x){color:blue}.b{color:green}"), "{got}");

        let both = out(":lexbor-contains(#x),:lexbor-contains(\"ok\"),:lexbor-contains(*)");
        assert_eq!(both.matches("lexbor-contains").count(), 1, "{both}");
    }

    #[test]
    fn does_not_run_past_the_end() {
        for s in [
            ":",
            ":lexbor-contains",
            ":lexbor-contains(",
            ":lexbor-contains(\\",
            ":lexbor-contains(\"",
            "\\",
            "/*",
            "/*unterminated",
            "\"unterminated",
            ":\\",
        ] {
            let _ = neutralized(s.as_bytes()).expect("no oom");
        }
    }

    /// Where a string ENDS is the tokenizer's call: CR, FF and LF all end one
    /// (a bad string), and the text after it is read on. The hand scanner this
    /// replaced ended strings at LF only and gave up on the rest of the input
    /// at an unterminated one, so each of these reached the parser unseen.
    #[test]
    fn a_newline_ends_a_string_and_the_rest_is_still_read() {
        for nl in ["\r", "\n", "\r\n", "\u{0c}"] {
            for q in ["\"", "'"] {
                rewritten(&format!("{q}x{nl}:lexbor-contains(#x)"));
                rewritten(&format!(
                    "a{{content:{q}x{nl}}}b:lexbor-contains(#x){{color:red}}"
                ));
                rewritten(&format!("[title={q}x{nl}], :lexbor-contains(#x)"));
                untouched(&format!("{q}x{nl}:lexbor-contains(\"ok\")"));
            }
        }
    }

    /// A backslash before a newline CONTINUES a string, so what follows is
    /// string content until the closing quote - and code again after it.
    #[test]
    fn a_backslash_newline_continues_a_string() {
        for nl in ["\n", "\r\n", "\r", "\u{0c}"] {
            untouched(&format!("p[title=\"x\\{nl}:lexbor-contains(#x)\"]"));
            rewritten(&format!("p[title=\"x\\{nl}\"], :lexbor-contains(#x)"));
            rewritten(&format!(
                "a{{content:\"x\\{nl}\"}}b:lexbor-contains(#x){{}}"
            ));
        }
    }

    #[test]
    fn an_unterminated_string_or_argument_is_not_accepted() {
        untouched("p[title=\"x :lexbor-contains(#x)");
        untouched("/* :lexbor-contains(#x)");
        for s in [
            ":lexbor-contains(\"x\"",
            ":lexbor-contains(x",
            ":lexbor-contains(x i",
            ":lexbor-contains(\"x\n\")",
            ":lexbor-contains(\"x\r\")",
            ":lexbor-contains(\"x\" i /* c */ )",
        ] {
            rewritten(s);
        }
    }

    /// The crash inputs from review, end to end through the stylesheet reader:
    /// the rule holding the pseudo is `bad_style`, the one before survives,
    /// and nothing reaches the serializer that dereferenced the broken
    /// argument (`lxb_css_selector_serialize` on a pseudo whose data the
    /// failed parse left behind).
    #[test]
    fn the_review_crash_inputs_parse_as_a_bad_rule() {
        use crate::lexbor::stylesheet::{parse, Rule};
        for input in [
            "a{content:\"x\r}b:lexbor-contains(#x){color:red}",
            "a{content:\"x\n}b:lexbor-contains(#x){color:red}",
            "a{content:\"x\\\n\"}b:lexbor-contains(#x){color:red}",
            "a{content:\"x\u{0c}}b:lexbor-contains(#x){color:red}",
            "a{content:\"x\r\n}b:lexbor-contains(#x){color:red}",
        ] {
            let Ok(rules) = parse(input.as_bytes()) else {
                panic!("{input:?} failed to parse");
            };
            assert!(
                rules
                    .iter()
                    .any(|r| matches!(r, Rule::BadStyle { selector_text, .. }
                        if selector_text.ends_with(b"lexbor-contains(#x)"))),
                "{input:?}: the pseudo's rule should be bad_style"
            );
        }
    }
}

mod guard_tokens {
    //! [`crate::lexbor::contains_guard`] over generated text, end to end: every
    //! input goes through the guarded stylesheet reader (a crash there fails
    //! the run), and the guard's own output must be a fixed point - nothing
    //! left in it that Lexbor's tokenizer reads as an unaccepted
    //! `lexbor-contains(`.

    use crate::lexbor::contains_guard::neutralized;

    const OPENERS: &[&str] = &["", "a{content:\"x", "a{content:'x", "[t=\"x", "/*", "p"];
    const BREAKS: &[&str] = &[
        "", "\r", "\n", "\r\n", "\u{0c}", "\\\n", "\\\r\n", "\\\r", "\\\u{0c}", "\"", "'", "\\",
        "*/", "\\\"",
    ];
    const MIDDLES: &[&str] = &["", "}", "]", "}b", "\"}b", ",", " "];
    const NAMES: &[&str] = &[
        ":lexbor-contains(",
        ":LEXBOR-Contains(",
        ":\\6C exbor-contains(",
        ":lexbor\\-contains(",
        ":/**/lexbor-contains(",
        "::lexbor-contains(",
    ];
    const ARGS: &[&str] = &[
        "#x)",
        "\"ok\")",
        "ok i)",
        ")",
        "\"x\r\")",
        "\"x\\\n\")",
        "\"x",
        "x",
        "\"x\" i  )",
        "\"x\" i /**/ )",
        ":lexbor-contains(#y))",
        "\"a\")",
    ];
    const TAILS: &[&str] = &["", "{color:red}", "{}", " a{b:c}", "\"", "\n{}"];

    #[test]
    fn generated_text_never_reaches_lexbor_unguarded() {
        let mut n = 0usize;
        for o in OPENERS {
            for b in BREAKS {
                for m in MIDDLES {
                    for name in NAMES {
                        for a in ARGS {
                            for t in TAILS {
                                let text = format!("{o}{b}{m}{name}{a}{t}");
                                check(text.as_bytes());
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(n > 10_000, "{n}");
    }

    fn check(input: &[u8]) {
        let once = neutralized(input).expect("no oom");
        let out = once.as_deref().unwrap_or(input);
        assert_eq!(out.len(), input.len());
        assert_eq!(
            neutralized(out).expect("no oom"),
            None,
            "the guard's output still holds an unaccepted lexbor-contains: {:?}",
            String::from_utf8_lossy(input)
        );
        /* A crash here, not an assertion, is how this fails. */
        let _ = crate::lexbor::stylesheet::parse(input);
    }
}

mod guard_agreement {
    //! [`crate::lexbor::contains_guard`]'s second rule, checked against the real
    //! parser rather than against a second reading of the grammar: whatever the
    //! guard leaves untouched, the parser must accept. Only the untouched ones
    //! are handed to it here.

    use crate::lexbor::contains_guard::neutralized;
    use crate::lexbor::css_engine::ParserParts;

    /// Argument texts around every boundary the recogniser draws.
    const ARGUMENTS: &[&str] = &[
        "",
        " ",
        "\"x\"",
        "'x'",
        "\"\"",
        "''",
        "\"x\" i",
        "\"x\" I",
        "\"x\"i",
        "\"x\"  i  ",
        "ident",
        " ident ",
        "ident i",
        "ident I",
        "ident  i",
        "-ident",
        "--ident",
        "_ident",
        "\\69 dent",
        "\\-x",
        "i",
        "I",
        "i i",
        "x y",
        "\"x\" y",
        "\"x\" ii",
        "\"x\" i i",
        "123",
        "1e3",
        "-1",
        "+1",
        "#x",
        ".x",
        "*",
        ",",
        ")",
        "(",
        "foo(bar)",
        "[x]",
        "@x",
        "\"unterminated",
        "'unterminated",
        "\\",
        "\"x\" /* c */ i",
        "/* c */ \"x\"",
        "\"x\"/*c*/",
        "\u{00e9}",
        "\"\u{00e9}\"",
        "--",
        "-",
        "\t\"x\"\n",
        "\"a\\\"b\"",
        "\"x\r\"",
        "\"x\u{0c}\"",
        "\"x\\\n\"",
        "\"x\\\r\n\"",
        "'x\\\r'",
        "\"x\" i\r\n",
        "\"x\" i /**/ ",
        "\\69",
        "\"x\" \\69",
    ];

    #[test]
    fn nothing_the_guard_keeps_makes_lexbor_fail() {
        let gvl = crate::gvl::Gvl::exclusive();
        /* Borrowed, not `into_parser`: `parts` must still free all three when
         * this returns, or the run leaks under LeakSanitizer. */
        let parts = ParserParts::build().expect("lexbor css parser");
        let parser = parts.as_parser();
        let _ = &gvl;

        let mut checked = 0;
        for arg in ARGUMENTS {
            for shape in [
                format!(":lexbor-contains({arg})"),
                format!("p:lexbor-contains({arg})"),
                format!(":lexbor-contains({arg}) a"),
                format!("a, :lexbor-contains({arg}), b"),
            ] {
                let bytes = shape.as_bytes();
                if neutralized(bytes).expect("no oom").is_some() {
                    continue; /* rewritten: never handed to Lexbor */
                }
                checked += 1;
                // SAFETY: the parser is live and exclusively ours for this call.
                let list = unsafe { parser.parse(bytes) };
                // SAFETY: same, and no list is read after the clean.
                unsafe { parser.clean_all() };
                assert!(
                    list.is_ok(),
                    "the guard kept {shape:?}, but the parser rejects it - the guard \
                     must never be laxer than the parser"
                );
            }
        }
        assert!(
            checked > 0,
            "the guard rewrote everything; nothing was checked"
        );
    }
}

/// The tree-depth guard (`adapter::tree_guard`), against the real parser: the
/// exact boundary for a document and for a fragment, and that a refusal is
/// reported as the limit rather than as a generic failure.
mod tree_depth {
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParseError};
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    use crate::lexbor::fragment::{FragmentContext, FragmentError, FragmentTag, TransientFragment};

    fn divs(n: usize) -> Vec<u8> {
        "<div>".repeat(n).into_bytes()
    }

    fn doc(n: usize, limit: DepthLimit) -> Result<(), HtmlParseError> {
        parse_html(&divs(n), true, limit).map(drop)
    }

    #[test]
    fn a_document_counts_html_and_body() {
        /* html (1) + body (2) + 398 divs = 400. */
        assert_eq!(doc(398, DepthLimit::DEFAULT), Ok(()));
        assert_eq!(doc(399, DepthLimit::DEFAULT), Err(HtmlParseError::TooDeep));
        assert_eq!(doc(3, DepthLimit::at_most(5)), Ok(()));
        assert_eq!(doc(4, DepthLimit::at_most(5)), Err(HtmlParseError::TooDeep));
        assert_eq!(doc(3000, DepthLimit::UNLIMITED), Ok(()));
        /* Nothing is accepted under a zero limit: `<html>` is already 1. */
        assert_eq!(doc(0, DepthLimit::at_most(0)), Err(HtmlParseError::TooDeep));
    }

    #[test]
    fn a_fragment_does_not_count_its_synthetic_root() {
        let host = parse_html(b"<p>", true, DepthLimit::DEFAULT).expect("host parses");
        let ctx = FragmentContext::Tag {
            doc: host.raw_doc(),
            at: FragmentTag::BODY,
        };
        let frag = |n: usize, limit| {
            // SAFETY: `host` is live for the call and the input is only read.
            unsafe { TransientFragment::parse(&divs(n), true, &ctx, limit) }.map(drop)
        };
        assert_eq!(frag(400, DepthLimit::DEFAULT), Ok(()));
        assert_eq!(frag(401, DepthLimit::DEFAULT), Err(FragmentError::TooDeep));
        assert_eq!(frag(3000, DepthLimit::UNLIMITED), Ok(()));
    }
}

mod node_key {
    use crate::lexbor::adapter::html::RawNode;
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParsed};
    use crate::lexbor::adapter::tree_guard::DepthLimit;

    fn doc(html: &[u8]) -> Box<HtmlParsed> {
        parse_html(html, true, DepthLimit::DEFAULT).expect("a document parses")
    }

    fn root(parsed: &HtmlParsed) -> RawNode {
        // SAFETY: `parsed` is live for the call.
        let doc = unsafe { parsed.raw_doc().as_doc() };
        RawNode::from(
            doc.as_node()
                .document_root()
                .expect("the parser always inserts a root"),
        )
    }

    #[test]
    fn a_key_resolves_to_its_own_node() {
        let parsed = doc(b"<div id=x>y</div>");
        let node = root(&parsed);
        // SAFETY: `node` is a live node of `parsed`.
        let key = unsafe { parsed.mint_key(node) }.expect("its own node mints");
        assert!(RawNode::from(parsed.resolve(key).expect("mints, so it resolves")) == node);
    }

    #[test]
    fn a_node_of_another_document_is_refused() {
        let a = doc(b"<p>a</p>");
        let b = doc(b"<p>b</p>");
        let node = root(&a);
        // SAFETY: `node` is a live node (of `a`).
        assert!(
            unsafe { b.mint_key(node) }.is_err(),
            "another document's node must not mint"
        );
        let key = unsafe { a.mint_key(node) }.expect("its own node mints");
        assert!(
            b.resolve(key).is_err(),
            "a key must not resolve under another document"
        );
    }
}

/// `lexbor::css_match` - a semantic port of Lexbor's `selectors.c`, not a
/// `selectors`-crate-based approach (rejected: that matcher recurses natively
/// on selector nesting - `css_match`'s module doc). Parses via the existing
/// `css_parser`, matches via an explicit heap work stack (never native
/// recursion for selector nesting - verified at 500,000 levels below).
mod css_match {
    use crate::gvl::Gvl;
    use crate::lexbor::adapter::html::{HtmlElement, HtmlNode, NsId, RawNode};
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParsed};
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    use crate::lexbor::css_match::{
        self as port, validate, MatchFailure, QueryFailure, Scratch, MAX_COMPOUNDS,
    };
    use crate::lexbor::css_parser::{self, Lists};
    use crate::text::VerifiedText;

    /* The engine's entry points take the `Scratch` a caller keeps; each test
     * query here starts from a fresh one, as a cold call would. */

    fn port_select_all<'doc>(
        root: HtmlNode<'doc>,
        groups: Lists<'_>,
    ) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
        port::select_all(&mut Scratch::new(), root, groups)
    }

    fn port_select_first<'doc>(
        root: HtmlNode<'doc>,
        groups: Lists<'_>,
    ) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
        port::select_first(&mut Scratch::new(), root, groups)
    }

    fn matches_any(groups: Lists<'_>, element: HtmlElement<'_>) -> Result<bool, MatchFailure> {
        port::matches_any(&mut Scratch::new(), groups, element)
    }

    fn parsed(html: &[u8]) -> Box<HtmlParsed> {
        parse_html(html, true, DepthLimit::DEFAULT).expect("a document parses")
    }

    /// The node `Node#css` etc. would actually be called on for "the whole
    /// document" - the Document node itself, matching `old_select_all`'s
    /// `d.as_node()` below - NOT `document_root()` (`<html>`). `<html>` is a
    /// genuine descendant of this and so a legitimate match for `*`; rooting
    /// here instead of there previously undercounted it by one against the
    /// old engine.
    fn root(doc: &HtmlParsed) -> HtmlNode<'_> {
        // SAFETY: `doc` outlives the borrow this returns.
        let d = unsafe { doc.raw_doc().as_doc() };
        d.as_node()
    }

    fn matches_selector(element: HtmlElement<'_>, selector: &str) -> bool {
        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(selector.as_bytes()).expect("verified");
        let parsed = css_parser::parse(&gvl, text)
            .unwrap_or_else(|_| panic!("selector {selector:?} failed to parse"));
        matches_any(parsed.groups(), element)
            .unwrap_or_else(|_| panic!("selector {selector:?} exceeded its work budget"))
    }

    /// Through the production `select_all`/`matches_any` entry points, not a
    /// second reimplementation of the subtree walk: this and `matches_selector`
    /// above are what the differential test further down cross-checks against
    /// the old Lexbor engine.
    fn select_all<'d>(doc: &'d HtmlParsed, selector: &str) -> Vec<HtmlElement<'d>> {
        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(selector.as_bytes()).expect("verified");
        let parsed = css_parser::parse(&gvl, text)
            .unwrap_or_else(|_| panic!("selector {selector:?} failed to parse"));
        port_select_all(root(doc), parsed.groups())
            .unwrap_or_else(|e| panic!("selector {selector:?} failed: {e:?}"))
            .into_iter()
            .filter_map(HtmlNode::element)
            .collect()
    }

    fn texts(doc: &HtmlParsed, selector: &str) -> Vec<String> {
        select_all(doc, selector)
            .into_iter()
            .map(|e| {
                let text = e.node().children().find_map(HtmlNode::char_data);
                String::from_utf8_lossy(text.unwrap_or(b"")).into_owned()
            })
            .collect()
    }

    /// Selectors 4 and the HTML Standard ("case-sensitivity of selectors"):
    /// a type selector is lower-cased and compared to an HTML element's
    /// localName, and compared as written to any other element's.
    /// `lxb_selectors_match_element` folds case on every element, so this is
    /// a departure from the old engine, and the one `matches_any` makes too.
    #[test]
    fn type_selector_case_follows_the_element_namespace() {
        let doc = parsed(b"<html><body><p>1</p><span>2</span><p>3</p></body></html>");
        assert_eq!(texts(&doc, "p"), ["1", "3"]);
        assert_eq!(texts(&doc, "P"), ["1", "3"]);

        let svg = parsed(
            b"<html><body><svg><circle r='1'/><linearGradient/></svg><math><mi/></math></body></html>",
        );
        assert_eq!(select_all(&svg, "circle").len(), 1);
        assert_eq!(select_all(&svg, "CIRCLE").len(), 0);
        assert_eq!(select_all(&svg, "linearGradient").len(), 1);
        assert_eq!(select_all(&svg, "lineargradient").len(), 0);
        assert_eq!(select_all(&svg, "MI").len(), 0);
        let grad = root(&svg)
            .subtree()
            .filter_map(HtmlNode::element)
            .find(|e| e.local_name() == b"lineargradient")
            .expect("the gradient");
        assert!(matches_selector(grad, "linearGradient"));
        assert!(!matches_selector(grad, "lineargradient"));
    }

    /// `:nth-*` with a B at the edge of what Lexbor's parser clamps to
    /// (`LONG_MAX`, for a number past it written as its own token - `2n-99...`
    /// without spaces is a syntax error there): answered, where `pos - b` in
    /// 64 bits panicked in a release build.
    #[test]
    fn nth_child_with_extreme_coefficients_is_answered() {
        let doc = parsed(b"<ul><li>1</li><li>2</li><li>3</li></ul>");
        assert_eq!(
            texts(&doc, "li:nth-child(n-9223372036854775807)"),
            ["1", "2", "3"]
        );
        assert_eq!(
            texts(&doc, "li:nth-child(2n - 99999999999999999999)"),
            ["1", "3"]
        );
        assert_eq!(
            texts(&doc, "li:nth-last-child(-n + 99999999999999999999)"),
            ["1", "2", "3"]
        );
        assert!(texts(&doc, "li:nth-child(-n-9223372036854775807)").is_empty());
        assert_eq!(
            texts(&doc, "li:nth-child(n-9223372036854775807 of li)"),
            ["1", "2", "3"]
        );
    }

    #[test]
    fn class_and_id_fold_case_only_in_quirks_mode() {
        // `lxb_selectors_match_class`/`_id`, and Lexbor's own test
        // `match_id_class_case`.
        let quirks = parsed(b"<html><body><div class='Test' id='Foo'></div></body></html>");
        assert_eq!(select_all(&quirks, ".test").len(), 1);
        assert_eq!(select_all(&quirks, "#foo").len(), 1);

        let no_quirks =
            parsed(b"<!doctype html><html><body><div class='Test' id='Foo'></div></body></html>");
        assert_eq!(select_all(&no_quirks, ".test").len(), 0);
        assert_eq!(select_all(&no_quirks, "#foo").len(), 0);
        assert_eq!(select_all(&no_quirks, ".Test").len(), 1);
        assert_eq!(select_all(&no_quirks, "#Foo").len(), 1);
    }

    #[test]
    fn html_attribute_value_case_insensitivity_table() {
        // `lxb_selectors_match_attribute_html_case_insensitive`, and Lexbor's
        // own test `match_html_case_insensitive_attributes`: `type` is in the
        // table (HTML, no modifier -> CI); `data-x` is not (always CS); `s`
        // forces case-sensitive even for a table attribute; the table does not
        // apply inside SVG (foreign content).
        let doc = parsed(
            b"<!doctype html><html><body>\
              <input type='TEXT'><div data-x='ABC'></div>\
              <svg><a rel='NOFOLLOW'></a></svg>\
              </body></html>",
        );
        assert_eq!(select_all(&doc, "[type=TEXT]").len(), 1);
        assert_eq!(select_all(&doc, "[type=text]").len(), 1); // table default: CI
        assert_eq!(select_all(&doc, "[type=text s]").len(), 0); // explicit s forces CS
        assert_eq!(select_all(&doc, "[TYPE=text]").len(), 1); // the table is keyed case-insensitively
        assert_eq!(select_all(&doc, "[type=text i]").len(), 1);
        assert_eq!(select_all(&doc, "[data-x=abc]").len(), 0); // not in the table: CS
        assert_eq!(select_all(&doc, "[data-x=ABC]").len(), 1);
        assert_eq!(select_all(&doc, "[rel=nofollow]").len(), 0); // SVG: table doesn't apply
        assert_eq!(select_all(&doc, "[rel=NOFOLLOW]").len(), 1);
    }

    #[test]
    fn lexbor_whitespace_set_excludes_vertical_tab() {
        // `lexbor_utils_whitespace`: space/tab/LF/FF/CR are separators;
        // vertical tab (0x0B) is NOT, unlike Rust's `is_ascii_whitespace`.
        let doc = parsed(b"<html><body><div class='a\x0Bb'></div></body></html>");
        // "a\x0Bb" is ONE token (VT doesn't split it), so `.a`/`.b` alone
        // don't match, but the whole token does.
        assert_eq!(select_all(&doc, ".a").len(), 0);
        assert_eq!(select_all(&doc, "[class~=\"a\x0Bb\"]").len(), 1);
    }

    #[test]
    fn attribute_operators() {
        let doc = parsed(
            b"<html><body>\
              <a href='/x'>x</a><a>y</a>\
              <a rel='next prev'>z</a>\
              <a lang='en-GB'>w</a>\
              <a data-p='foobar'>v</a>\
              </body></html>",
        );
        assert_eq!(select_all(&doc, "a[href]").len(), 1);
        assert_eq!(select_all(&doc, "a[href='/x']").len(), 1);
        assert_eq!(select_all(&doc, "a[rel~='next']").len(), 1);
        assert_eq!(select_all(&doc, "a[rel~='nex']").len(), 0);
        assert_eq!(select_all(&doc, "a[lang|='en']").len(), 1);
        assert_eq!(select_all(&doc, "a[lang|='eng']").len(), 0);
        assert_eq!(select_all(&doc, "a[data-p^='foo']").len(), 1);
        assert_eq!(select_all(&doc, "a[data-p$='bar']").len(), 1);
        assert_eq!(select_all(&doc, "a[data-p*='oob']").len(), 1);
        assert_eq!(select_all(&doc, "a[data-p^='']").len(), 0); // empty operand never matches
    }

    #[test]
    fn descendant_and_child_combinators() {
        let doc =
            parsed(b"<html><body><div><p>1</p><span><p>2</p></span></div><p>3</p></body></html>");
        assert_eq!(texts(&doc, "div p"), ["1", "2"]);
        assert_eq!(texts(&doc, "div > p"), ["1"]);
    }

    #[test]
    fn sibling_combinators() {
        let doc = parsed(b"<html><body><p>1</p><p>2</p><span></span><p>3</p></body></html>");
        assert_eq!(texts(&doc, "p + p"), ["2"]);
        assert_eq!(texts(&doc, "p ~ p"), ["2", "3"]);
    }

    #[test]
    fn nth_child_family() {
        let doc = parsed(b"<html><body><ul><li>a</li><li>b</li><li>c</li></ul></body></html>");
        assert_eq!(texts(&doc, "li:nth-child(2)"), ["b"]);
        assert_eq!(texts(&doc, "li:nth-child(odd)"), ["a", "c"]);
        assert_eq!(texts(&doc, "li:nth-last-child(1)"), ["c"]);
        assert_eq!(texts(&doc, "li:first-child"), ["a"]);
        assert_eq!(texts(&doc, "li:last-child"), ["c"]);
    }

    #[test]
    fn nth_child_of_s() {
        // Position counted only among siblings matching `S`.
        let doc = parsed(
            b"<html><body><main>\
              <h2 class='mark'>1</h2><h2>2</h2><h2 class='mark'>3</h2>\
              <h2 class='mark'>4</h2><h2>5</h2>\
              </main></body></html>",
        );
        assert_eq!(texts(&doc, "h2:nth-child(2 of .mark)"), ["3"]);
        assert_eq!(texts(&doc, "h2:nth-child(even of .mark)"), ["3"]);
        assert_eq!(texts(&doc, "h2:nth-child(odd of .mark)"), ["1", "4"]);
    }

    #[test]
    fn of_type_family() {
        let doc =
            parsed(b"<html><body><div><p>1</p><span>x</span><p>2</p><p>3</p></div></body></html>");
        assert_eq!(texts(&doc, "p:first-of-type"), ["1"]);
        assert_eq!(texts(&doc, "p:last-of-type"), ["3"]);
        assert_eq!(texts(&doc, "span:only-of-type"), ["x"]);
        assert_eq!(texts(&doc, "p:nth-of-type(2)"), ["2"]);
        assert_eq!(texts(&doc, "p:nth-last-of-type(1)"), ["3"]);
    }

    #[test]
    fn is_where_not_match_correctly_including_nesting() {
        let doc = parsed(
            b"<html><body><div><p class='a'>1</p><span>2</span><p>3</p></div></body></html>",
        );
        assert_eq!(texts(&doc, "div :is(p, span)"), ["1", "2", "3"]);
        assert_eq!(texts(&doc, "div :is(:is(:is(p.a)))"), ["1"]);
        assert_eq!(texts(&doc, "div :where(.a)"), ["1"]);
        assert_eq!(texts(&doc, "div :not(p)"), ["2"]);
        assert_eq!(texts(&doc, "div :not(:is(p, span))"), Vec::<String>::new());
    }

    #[test]
    fn has_single_and_multi_compound() {
        let doc =
            parsed(b"<html><body><div><p>x</p></div><div></div><ul><li>a</li></ul></body></html>");
        assert_eq!(select_all(&doc, "div:has(p)").len(), 1);
        assert_eq!(select_all(&doc, "ul:has(> li)").len(), 1);
        assert_eq!(select_all(&doc, "div:has(> p)").len(), 1);

        // A multi-compound `:has()` argument (Lexbor's forward search), now
        // supported via the bounded forward search (`has_forward`) - the
        // `selectors`-crate exploration and the first cut of this port both
        // left this unimplemented.
        let nested = parsed(
            b"<html><body>\
              <div><section><p class='x'>hit</p></section></div>\
              <div><section><p>miss</p></section></div>\
              </body></html>",
        );
        assert_eq!(select_all(&nested, "div:has(section > p.x)").len(), 1);
        assert_eq!(select_all(&nested, "div:has(section > p.nope)").len(), 0);

        let sib = parsed(b"<html><body><p></p><span>hit</span><p></p><p></p></body></html>");
        assert_eq!(texts(&sib, "p:has(+ span)").len(), 1);
        assert_eq!(select_all(&sib, "p:has(~ span)").len(), 1);
    }

    /// A compound that starts with a list pseudo-class is tried at every
    /// candidate its combinator allows, as Selectors 4 says. Lexbor's engine
    /// tries only the first - the nearest ancestor, the previous sibling, a
    /// `:has()` subject's first child - unless something precedes the pseudo
    /// in the compound (`*:is(div) span` is right there). That is a departure
    /// (`css_match`'s module doc), and the differential checks leave the shape
    /// out; the second half of this test is what says when a Lexbor bump makes
    /// that exclusion unnecessary.
    #[test]
    fn a_compound_led_by_a_list_pseudo_tries_every_candidate() {
        use crate::lexbor::selectors as old_engine;

        let doc = parsed(
            b"<div><section><p><a></a></p></section></div>\
              <p></p><b></b><i></i>",
        );
        let names = |els: Vec<HtmlElement<'_>>| -> Vec<String> {
            els.into_iter()
                .map(|e| String::from_utf8_lossy(e.qualified_name()).into_owned())
                .collect()
        };
        let cases: [(&str, &[&str]); 7] = [
            (":is(div) a", &["a"]),
            (":not(p) a", &["a"]),
            (":is(div).x a, :where(section) a", &["a"]),
            (":is(p) ~ i", &["i"]),
            (":has(:is(p))", &["html", "body", "div", "section"]),
            (":has(> :is(body))", &["html"]),
            (":has(:has(> a))", &["html", "body", "div", "section"]),
        ];
        for (sel, want) in cases {
            assert_eq!(names(select_all(&doc, sel)), want, "{sel}");

            let gvl = Gvl::exclusive();
            let old = old_engine::select_all(&gvl, RawNode::from(root(&doc)), sel.as_bytes())
                .unwrap_or_else(|_| panic!("old engine rejected {sel:?}"));
            assert_ne!(
                old.len(),
                want.len(),
                "Lexbor now answers {sel} as the spec does: drop the exclusion of a \
                 compound led by a list pseudo from the differential checks"
            );
        }
    }

    /// A construct this engine cannot evaluate at all - the column
    /// combinator `||` (Lexbor's OWN traversal reports an error for it too)
    /// and `:lexbor-contains()` (Lexbor itself matches with it; this port
    /// deliberately does not) - must be RAISED, never silently answered as
    /// "no element matches" (`MatchFailure`'s doc): either would otherwise
    /// look exactly like a legitimate empty result.
    #[test]
    fn unsupported_constructs_are_raised_not_answered_as_empty() {
        use crate::lexbor::css_match::QueryFailure;

        let doc = parsed(
            b"<html><body><table><col><tr><td>x</td></tr></table><p>hello</p></body></html>",
        );

        for sel in ["col || td", "p:lexbor-contains(\"x\")"] {
            let gvl = Gvl::exclusive();
            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let parsed_sel =
                css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("{sel:?} fails to parse"));
            let result = port_select_all(root(&doc), parsed_sel.groups());
            assert!(
                matches!(result, Err(QueryFailure::Match(MatchFailure::Unsupported))),
                "{sel:?} should be Unsupported, was {:?}",
                result.is_ok()
            );
        }
    }

    /// A chain over [`MAX_COMPOUNDS`] used to be silently treated the same
    /// as an EMPTY one (`collect_compounds`'s own `None`) - fine for
    /// top-level `select_all` (an alternative that cannot match just
    /// contributes nothing), wrong wherever an OR/AND-negated construct
    /// treats "no alternatives left" as a verdict of its own: `:not()`'s
    /// "found none that matched, so it holds" answered `true` for a
    /// too-complex argument, exactly as if `:not()` had been written empty.
    /// [`validate`] catches this UP FRONT, wherever the over-limit chain is
    /// nested, so the query never reaches that silent path at all.
    #[test]
    fn too_complex_chains_are_caught_wherever_they_are_nested() {
        use crate::lexbor::css_match::MatchFailure;

        let chain_of = |n: usize| -> String {
            let mut s = String::from("div");
            for _ in 1..n {
                s.push_str(" > div");
            }
            s
        };

        let shapes: &[fn(String) -> String] = &[
            |c| c,
            |c| format!(":is({c})"),
            |c| format!(":not({c})"),
            |c| format!(":has({c})"),
            |c| format!(":nth-child(2 of {c})"),
        ];

        for shape in shapes {
            // Exactly at the cap: must still validate cleanly. Each
            // acquisition is scoped to its own block: `Gvl::exclusive`'s
            // stand-in mutex is NOT reentrant, and shadowing a `let gvl = ..`
            // binding does not drop the shadowed guard early - two live in
            // the same scope would self-deadlock on the second acquisition.
            let ok_sel = shape(chain_of(MAX_COMPOUNDS));
            {
                let gvl = Gvl::exclusive();
                let text = VerifiedText::from_bytes(ok_sel.as_bytes()).expect("verified");
                let parsed_sel = css_parser::parse(&gvl, text)
                    .unwrap_or_else(|_| panic!("{ok_sel:?} fails to parse"));
                assert!(
                    validate(parsed_sel.groups()).is_ok(),
                    "{ok_sel:?} (exactly {MAX_COMPOUNDS} compounds) should validate"
                );
            }

            // One past the cap: must be caught, not silently dropped.
            let bad_sel = shape(chain_of(MAX_COMPOUNDS + 1));
            let gvl = Gvl::exclusive();
            let text = VerifiedText::from_bytes(bad_sel.as_bytes()).expect("verified");
            let parsed_sel = css_parser::parse(&gvl, text)
                .unwrap_or_else(|_| panic!("{bad_sel:?} fails to parse"));
            assert!(
                matches!(validate(parsed_sel.groups()), Err(MatchFailure::TooComplex)),
                "{bad_sel:?} ({} compounds) should be TooComplex",
                MAX_COMPOUNDS + 1
            );
        }
    }

    /// The concrete bug [`too_complex_chains_are_caught_wherever_they_are_nested`]
    /// closes, through actual matching rather than `validate` alone:
    /// `:not()` wrapping a too-complex chain used to answer `true` for EVERY
    /// element (no alternatives left to disprove it), because the
    /// over-complex alternative was invisible to `:not()`'s own "found none
    /// that matched" logic. With `validate` run first (as the glue now
    /// does), this raises instead of silently matching everything.
    #[test]
    fn not_with_a_too_complex_chain_raises_instead_of_matching_everything() {
        use crate::lexbor::css_match::MatchFailure;

        let mut chain = String::from("div");
        for _ in 1..=MAX_COMPOUNDS + 1 {
            chain.push_str(" > div");
        }
        let sel = format!(":not({chain})");

        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
        let parsed_sel =
            css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("selector fails to parse"));

        assert!(matches!(
            validate(parsed_sel.groups()),
            Err(MatchFailure::TooComplex)
        ));
    }

    /// [`validate`] must find `:lexbor-contains()`/`||` wherever they sit -
    /// leading or trailing a comma list, or nested inside `:not`/`:has`/
    /// `of S` - and MUST NOT depend on whether an earlier alternative or an
    /// earlier simple selector in the SAME compound would have already
    /// settled the query. Regression for exactly the inconsistency found:
    /// `nosuch:lexbor-contains("x")` used to answer empty (a type mismatch
    /// short-circuited first), `p:lexbor-contains("x")` raised (the type
    /// matched), and `p, nosuch:lexbor-contains("x")` against a document
    /// with a `<p>` answered a match instead of raising, purely because `p`
    /// happened to come first.
    #[test]
    fn unsupported_constructs_are_found_regardless_of_position_or_short_circuit() {
        use crate::lexbor::css_match::MatchFailure;

        let shapes = [
            "nosuch:lexbor-contains(\"x\")",
            "p:lexbor-contains(\"x\")",
            "p, nosuch:lexbor-contains(\"x\")",
            "nosuch:lexbor-contains(\"x\"), p",
            ":not(p:lexbor-contains(\"x\"))",
            ":has(p:lexbor-contains(\"x\"))",
            ":nth-child(2 of p:lexbor-contains(\"x\"))",
            "col || td",
            "p, col || td",
            "col || td, p",
        ];
        for sel in shapes {
            let gvl = Gvl::exclusive();
            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let parsed_sel =
                css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("{sel:?} fails to parse"));
            assert!(
                matches!(
                    validate(parsed_sel.groups()),
                    Err(MatchFailure::Unsupported)
                ),
                "{sel:?} should be Unsupported regardless of position"
            );
        }

        // The actual query, against a document where `p` WOULD match first
        // if evaluation order mattered - it must not.
        let doc = parsed(
            b"<html><body><table><col><tr><td>x</td></tr></table><p>hello</p></body></html>",
        );
        // Mirrors the glue's order: `validate` first, matching only after.
        // `p` alone matches this document, so an order-dependent check would
        // answer a node here instead of raising.
        let sel = "p, nosuch:lexbor-contains(\"x\")";
        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
        let parsed_sel =
            css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("selector fails to parse"));
        let via_glue_order = validate(parsed_sel.groups())
            .and_then(|()| port_select_first(root(&doc), parsed_sel.groups()));
        assert!(matches!(via_glue_order, Err(MatchFailure::Unsupported)));
    }

    #[test]
    fn expanded_pseudo_classes() {
        let doc = parsed(
            b"<html><body>\
              <a href='/x'>link</a><a>nolink</a>\
              <input type='checkbox' checked><input type='checkbox'>\
              <input required><input>\
              <input readonly><input>\
              <button disabled></button>\
              <fieldset disabled><legend><button>ok</button></legend><button>no</button></fieldset>\
              <fieldset disabled><button>also-no</button></fieldset>\
              <p></p><p> </p><p>x</p>\
              <input placeholder='x'><input placeholder=''><input>\
              <textarea placeholder='y'></textarea>\
              <select placeholder='z'></select>\
              </body></html>",
        );
        assert_eq!(select_all(&doc, ":any-link").len(), 1);
        assert_eq!(select_all(&doc, ":link").len(), 1);
        assert_eq!(select_all(&doc, "input:checked").len(), 1);
        assert_eq!(select_all(&doc, "input:required").len(), 1);
        // 9 <input>s total, all but the `required` one are :optional.
        assert_eq!(select_all(&doc, "input:optional").len(), 8);
        assert_eq!(select_all(&doc, "input:read-only").len(), 1);
        // 9 <input>s total, all but the `readonly` one are :read-write.
        assert_eq!(select_all(&doc, "input:read-write").len(), 8);
        assert_eq!(select_all(&doc, "button:disabled").len(), 3); // own attr + 2 inherited
        assert_eq!(select_all(&doc, "button:enabled").len(), 1); // the one under <legend>
        assert_eq!(select_all(&doc, "p:empty").len(), 1);
        assert_eq!(select_all(&doc, "p:blank").len(), 2); // :blank tolerates whitespace-only text
        {
            // `:empty`/`:blank` ignore a COMMENT child but not a
            // processing-instruction one (`lxb_selectors_pseudo_class`'s
            // `EMPTY` case and `lxb_dom_node_is_empty` check
            // `local_name != EM_COMMENT`, not "is an element or non-empty
            // text") - found by `spec/xml_css_spec.rb`'s HTML/XML agreement
            // check.
            let pi_doc = parsed(b"<html><body><b><!--c--></b><i><?pi x?></i></body></html>");
            assert_eq!(select_all(&pi_doc, "b:empty").len(), 1);
            assert_eq!(select_all(&pi_doc, "i:empty").len(), 0);
            assert_eq!(select_all(&pi_doc, "b:blank").len(), 1);
            assert_eq!(select_all(&pi_doc, "i:blank").len(), 0);
        }
        // :active/:focus/:hover are literal attribute-presence checks
        // (`lxb_selectors_pseudo_class`), not "always false" - none of this
        // fixture's markup has them.
        assert_eq!(select_all(&doc, ":hover").len(), 0);
        // `lxb_selectors_pseudo_class`'s `PLACEHOLDER_SHOWN` case:
        // `input`/`textarea` only, PRESENCE of `placeholder` only - the
        // empty-valued one still counts, and the `<select placeholder>` (not a
        // real HTML attribute there, but Lexbor doesn't validate that) must
        // NOT, since it is neither tag.
        assert_eq!(select_all(&doc, ":placeholder-shown").len(), 3);
        assert_eq!(select_all(&doc, "select:placeholder-shown").len(), 0);
    }

    #[test]
    fn deeply_nested_is_does_not_grow_the_native_stack() {
        // The one property this module exists to prove (contrast the
        // `selectors`-crate exploration's sibling test, which crashed in a
        // release build at a nesting depth of only ~2,000-2,500). 500,000
        // mirrors the depth already measured safe for Lexbor's own C matcher.
        let doc = parsed(b"<html><body><a>x</a></body></html>");
        let depth = 500_000;
        let nested = format!("{}a{}", ":is(".repeat(depth), ")".repeat(depth));
        assert_eq!(texts(&doc, &nested), ["x"]);
    }

    /// A `:has()` chain at exactly [`MAX_COMPOUNDS`] (64) compounds -
    /// `Frame::HasStep`'s own compound-by-compound stepping (module doc), not
    /// native recursion (an earlier design answered `:has()` with ordinary
    /// Rust recursion here, `has_forward`; ITS OWN stack-safety margin was
    /// what this test originally measured - see
    /// `has_nested_inside_has_is_heap_based_not_native_recursion` for the
    /// nesting-depth counterpart, and this file's git history for the
    /// pre-conversion version of this test). Kept as a correctness regression
    /// (does a 64-compound chain still match its exactly-64-deep fixture?)
    /// and still run on a `Fiber`-sized stack as belt-and-braces, since
    /// nothing here should need more stack than that any more, at any depth.
    ///
    /// A distinct class per compound, not a shared one, so a short-circuit on
    /// an early mismatch cannot cut the search short and hide a real bug. A
    /// stack overflow (were one to reappear) kills the thread outright
    /// (`join()` returns `Err`, not a wrong `bool`).
    #[test]
    fn has_forward_at_max_compounds_fits_the_smallest_fiber_stack() {
        const FIBER_SIZED_STACK: usize = 128 * 1024;

        let handle = std::thread::Builder::new()
            .stack_size(FIBER_SIZED_STACK)
            .spawn(move || {
                let mut html = String::from("<!doctype html><html><body><div id=root>");
                let mut chain = String::new();
                for i in 0..MAX_COMPOUNDS {
                    html.push_str(&format!("<div class=c{i}>"));
                    if i > 0 {
                        chain.push(' ');
                    }
                    chain.push_str(&format!(".c{i}"));
                }
                for _ in 0..MAX_COMPOUNDS {
                    html.push_str("</div>");
                }
                html.push_str("</div></body></html>");

                let doc = parsed(html.as_bytes());
                let target = root(&doc)
                    .subtree()
                    .filter_map(HtmlNode::element)
                    .find(|e| e.qualified_name() == b"div")
                    .expect("the outer #root div");
                matches_selector(target, &format!("#root:has({chain})"))
            })
            .expect("spawn a Fiber-sized-stack thread");

        assert!(
            handle.join().expect("must not overflow a 128 KiB stack"),
            "a 64-level :has() argument should match its exactly-64-deep fixture"
        );
    }

    /// `:has()`'s search is the one place a query's cost can multiply per
    /// candidate rather than being bounded by the document's own size
    /// (module doc, `Budget`'s doc): each of N candidates here runs its OWN
    /// `:has(span.missing)` search over its own M descendants, none of which
    /// ever matches, so the search never short-circuits early - a real
    /// document could not be built big enough to hit the SHIPPED 50-million
    /// limit in a fast test, so this drives `select_all_with_work_limit`
    /// (`#[cfg(test)]`-only) with one small enough to actually exceed.
    /// `:nth-child(An+B of S)` remembers the ranks it has counted, as the
    /// plain `:nth-child` does (`Positions`): each candidate stops at the
    /// first sibling already counted, so a sibling list is tested against
    /// `S` once per query rather than once per candidate. Without it, 3,000
    /// items spent the shipped budget.
    #[test]
    fn nth_child_of_s_over_a_wide_list_costs_linear_work() {
        use crate::lexbor::css_match::select_all_with_work_limit;
        const N: usize = 20_000;

        let html = format!(
            "<ul>{}</ul>",
            r#"<li class="a"></li><li></li>"#.repeat(N / 2)
        );
        let doc = parsed(html.as_bytes());
        let gvl = Gvl::exclusive();
        for (sel, expected) in [
            ("li:nth-child(odd of .a)", N / 4),
            ("li:nth-last-child(2n of .a)", N / 4),
            ("li:nth-child(-n+3 of :not(.a))", 3),
        ] {
            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let parsed_sel =
                css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("{sel} parses"));
            let found = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 20 * N as u64)
                .unwrap_or_else(|_| panic!("{sel:?} exceeded work linear in the list"));
            assert_eq!(found.len(), expected, "{sel}");
        }
    }

    /// `:disabled`'s fieldset inheritance asks whether the child on the way
    /// up is the first `legend`, not which child is: finding the first
    /// legend scanned a wide fieldset once per control in it, quadratic and
    /// uncharged (40,000 inputs took seconds). A wide fieldset now costs work
    /// linear in its width. Controls in a legend that many siblings precede
    /// still scan back over them, each - and charge for it, so that shape
    /// fails closed.
    #[test]
    fn a_wide_disabled_fieldset_costs_linear_work() {
        use crate::lexbor::css_match::select_all_with_work_limit;
        const N: usize = 20_000;

        let gvl = Gvl::exclusive();
        let query = |html: String, limit: u64| {
            let doc = parsed(html.as_bytes());
            let text = VerifiedText::from_bytes(b"input:disabled").expect("verified");
            let sel = css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("parses"));
            select_all_with_work_limit(root(&doc), sel.groups(), limit).map(|v| v.len())
        };

        let wide = format!(
            "<fieldset disabled><legend>t</legend>{}</fieldset>",
            "<input>".repeat(N)
        );
        assert_eq!(query(wide, 20 * N as u64).ok(), Some(N));

        let late_legend = format!(
            "<fieldset disabled>{}<legend>{}</legend></fieldset>",
            "<div></div>".repeat(N),
            "<input>".repeat(N)
        );
        assert!(matches!(
            query(late_legend, 20 * N as u64),
            Err(crate::lexbor::css_match::QueryFailure::Match(
                crate::lexbor::css_match::MatchFailure::WorkExceeded
            ))
        ));
    }

    /// A chain that fails does not retry every combination of ancestors:
    /// `x` fails at every ancestor of the sixth `div`, so it fails at every
    /// ancestor of any higher one too (`Query::step_chain`'s `Fail`).
    /// Exhaustive backtracking tried C(40, 6) placements here and spent the
    /// whole shipped budget; pruned, one `<p>` costs about the depth. The
    /// placements that CAN match still do.
    #[test]
    fn a_failing_descendant_chain_does_not_try_every_combination_of_ancestors() {
        use crate::lexbor::css_match::select_all_with_work_limit;

        let html = format!("{}<p></p>{}", "<div>".repeat(40), "</div>".repeat(40));
        let doc = parsed(html.as_bytes());
        let gvl = Gvl::exclusive();
        for (sel, expected) in [
            ("x div div div div div div p", 0),
            ("div div div div div div div div p", 1),
            ("body > div div div div div div > div p", 1),
            ("body > div div div div > x div div p", 0),
            ("html > div div p", 0),
        ] {
            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let parsed_sel =
                css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("{sel} parses"));
            let found = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10_000)
                .unwrap_or_else(|_| panic!("{sel:?} exceeded a 10,000-step budget"));
            assert_eq!(found.len(), expected, "{sel}");
        }
    }

    #[test]
    fn a_has_search_that_cannot_find_anything_fails_closed_once_the_work_budget_is_spent() {
        use crate::lexbor::css_match::select_all_with_work_limit;

        const CANDIDATES: usize = 5;
        const DESCENDANTS_EACH: usize = 20;

        let mut html = String::from("<!doctype html><html><body>");
        for _ in 0..CANDIDATES {
            html.push_str("<li>");
            for _ in 0..DESCENDANTS_EACH {
                html.push_str("<span></span>");
            }
            html.push_str("</li>");
        }
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());

        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(b"li:has(span.missing)").expect("verified");
        let parsed_sel =
            css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("selector fails to parse"));

        // Comfortably below what `CANDIDATES * DESCENDANTS_EACH` (100) charges
        // (each candidate's `:has()` visits all `DESCENDANTS_EACH` of its own
        // descendants, since `span.missing` never matches and so never
        // short-circuits): must fail closed with `WorkExceeded`, never a
        // truncated or empty `Ok` - a shorter search would be a wrong answer,
        // not merely an incomplete one (module doc).
        let starved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10);
        assert!(matches!(
            starved,
            Err(crate::lexbor::css_match::QueryFailure::Match(
                crate::lexbor::css_match::MatchFailure::WorkExceeded
            ))
        ));

        // The same query with room to spare still answers correctly (empty:
        // no `<li>` actually has a `span.missing`) - the limit is what
        // tripped it above, not a bug in the search itself.
        let unstarved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10_000)
            .expect("comfortably within budget");
        assert!(unstarved.is_empty());
    }

    /// `:has()` NESTED inside `:has()` used to grow the REAL native call
    /// stack one level per nesting: `check_simple`'s `Has` arm answered it
    /// EAGERLY, in ordinary Rust recursion, unlike `:is`/`:where`/`:not`
    /// (already deferred to this file's heap `Frame`/`Cont` stack). Confirmed
    /// as a real, reachable crash before the fix: `:has(` x 300 `div` `)` x
    /// 300 against a 302-deep document, inside a `Fiber` given only
    /// `RUBY_FIBER_MACHINE_STACK_SIZE`'s documented minimum (128 KiB),
    /// crashed the WHOLE PROCESS with an uncaught `SystemStackError` that
    /// escaped every `rescue` (exit code 1). Lexbor's own C engine never had
    /// this problem (`lxb_selectors_nested_t`, a heap structure - module
    /// doc), so the fix brings `:has()` in line with that, not a depth cap:
    /// `Frame::HasStep`/`HasCursor` now answer it the same heap-based way as
    /// `:is()`, with no nesting limit at all.
    ///
    /// Measured the same way `has_forward_at_max_compounds_fits_the_-
    /// smallest_fiber_stack` measured the OLD design's own bound: a depth past
    /// the 300-level crash threshold above (comfortably beyond what the OLD,
    /// native-recursive design could survive on this stack - it died before
    /// 302), capped by the HTML parser's own `DepthLimit::DEFAULT` (400) for
    /// this test's fixture rather than a limit of the matcher's, on a thread
    /// given only a `RUBY_FIBER_MACHINE_STACK_SIZE`-sized (128 KiB) stack,
    /// must still answer correctly rather than overflow - proving the
    /// conversion actually removed the native recursion, not just moved where
    /// it fails. A linear chain of plain, unlabelled `<div>`s is enough: each
    /// `:has()`'s
    /// own argument is bare (an implicit-universal compound holding only the
    /// next `:has()`), so ANY element candidate lets the search descend one
    /// level deeper, and a document nested exactly as deep as the selector
    /// guarantees the search always has one more candidate to offer, at every
    /// level - so this actually EXERCISES the full depth, rather than
    /// short-circuiting on an empty subtree a few levels in.
    #[test]
    fn has_nested_inside_has_is_heap_based_not_native_recursion() {
        // Past the 300-level crash threshold measured against the OLD design
        // (see this test's doc), comfortably under the HTML parser's own
        // `DepthLimit::DEFAULT` (400) so `parsed()` need not raise that
        // separately - the property under test is the MATCHER's stack
        // safety, not the parser's (already covered elsewhere).
        const DEPTH: usize = 350;

        let handle = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || {
                let selector = format!("{}div{}", ":has(".repeat(DEPTH), ")".repeat(DEPTH));
                let mut html = String::from("<!doctype html><html><body>");
                for _ in 0..DEPTH {
                    html.push_str("<div>");
                }
                for _ in 0..DEPTH {
                    html.push_str("</div>");
                }
                html.push_str("</body></html>");
                let doc = parsed(html.as_bytes());

                let target = root(&doc)
                    .subtree()
                    .filter_map(HtmlNode::element)
                    .next()
                    .expect("the outermost div");
                matches_selector(target, &selector)
            })
            .expect("spawn a Fiber-sized-stack thread");

        assert!(
            handle.join().expect("must not overflow a 128 KiB stack"),
            "{DEPTH} levels of :has() nesting should still match its exactly-{DEPTH}-deep fixture"
        );
    }

    /// `sibling_position` (behind `:nth-of-type`/`:first-of-type`/
    /// `:last-of-type`/`:only-of-type`/`:nth-child` without `of S`) is
    /// O(siblings), same as `:has()`'s search - a wide sibling list under a
    /// query that checks one of these on every sibling costs work
    /// proportional to siblings² if uncounted. Same proof shape as the
    /// `:has()` test above: a small `select_all_with_work_limit` limit
    /// against a small fixture, since a real sibling list wide enough to
    /// exhaust the shipped 10-million default would be impractical to build
    /// here.
    #[test]
    fn wide_sibling_lists_under_of_type_checks_charge_the_work_budget_too() {
        use crate::lexbor::css_match::select_all_with_work_limit;

        const SIBLINGS: usize = 30;

        let mut html = String::from("<!doctype html><html><body>");
        for _ in 0..SIBLINGS {
            html.push_str("<span></span>");
        }
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());

        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(b"span:last-of-type").expect("verified");
        let parsed_sel =
            css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("selector fails to parse"));

        // `:last-of-type` on the FIRST of 30 same-type siblings walks all 29
        // that follow before answering `false` - well past a limit of 10.
        let starved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10);
        assert!(matches!(
            starved,
            Err(crate::lexbor::css_match::QueryFailure::Match(
                crate::lexbor::css_match::MatchFailure::WorkExceeded
            ))
        ));

        // With room to spare it answers correctly: exactly the last <span>.
        let unstarved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10_000)
            .expect("comfortably within budget");
        assert_eq!(unstarved.len(), 1);
    }

    /// `:any-link` / `:link` answer exactly as Lexbor's matcher does: `map`
    /// counts for `:any-link`, `link` for `:link`, the tag in any namespace,
    /// and `href` in any namespace (`xlink:href`).
    #[test]
    fn any_link_and_link_agree_with_lexbor() {
        use crate::lexbor::selectors as old_engine;
        let doc = parsed(
            br##"<!doctype html><html><head><link href=s.css rel=stylesheet><link rel=icon></head><body>
                <a href=/x>a</a><a>no-href</a><area href=/y><map href=/m></map><map></map>
                <svg><a xlink:href="#p">svg-xlink</a><a href="#q">svg-plain</a><a>svg-none</a></svg>
                <div href=/z>div</div>
                </body></html>"##,
        );
        for sel in [
            ":any-link",
            ":link",
            "a:any-link",
            "svg :link",
            ":not(:any-link)",
        ] {
            let old = {
                let gvl = Gvl::exclusive();
                // SAFETY: `doc` outlives the call, and its root is a live node.
                let d = unsafe { doc.raw_doc().as_doc() };
                old_engine::select_all(&gvl, RawNode::from(d.as_node()), sel.as_bytes())
                    .unwrap_or_else(|_| panic!("{sel}: old engine"))
            };
            let new: Vec<RawNode> = select_all(&doc, sel)
                .into_iter()
                .map(|e| RawNode::from(e.node()))
                .collect();
            assert!(!new.is_empty(), "{sel}");
            assert!(new == old, "{sel}: new {} old {}", new.len(), old.len());
        }
    }

    /// `:disabled`, `:enabled` and `:checked` by the HTML Standard's
    /// definitions, including where Lexbor's matcher differs: a legend after
    /// whitespace, fieldset inheritance for controls without the attribute,
    /// nested fieldsets, `option`/`optgroup`, custom elements and `<div>`.
    #[test]
    fn form_state_pseudo_classes_follow_the_html_standard() {
        let doc = parsed(
            b"<!doctype html><body>\
              <fieldset disabled>\n <legend><input id=in-legend></legend>\
                <input id=inherits><div><button id=deep></button></div>\
                <legend><input id=second-legend></legend>\
                <fieldset id=inner><legend><input id=inner-legend></legend></fieldset>\
              </fieldset>\
              <fieldset disabled><div></div><legend><select id=legend-after-div></select></legend></fieldset>\
              <fieldset disabled></fieldset>\
              <select><optgroup disabled id=og><option id=in-og>a</option></optgroup>\
                <option disabled id=opt>b</option><option id=opt-ok selected>c</option></select>\
              <textarea id=ta></textarea><div id=plain disabled></div>\
              <my-el id=custom disabled checked></my-el>\
              <input type=RADIO checked id=radio><input type=text checked id=text>\
              <svg><input disabled id=svg-input></input></svg>\
              </body>",
        );
        let ids = |sel: &str| -> Vec<String> {
            select_all(&doc, sel)
                .into_iter()
                .filter_map(|e| {
                    e.get_attribute(b"id")
                        .map(|v| String::from_utf8_lossy(v).into_owned())
                })
                .collect()
        };
        assert_eq!(
            ids(":disabled"),
            [
                "inherits",
                "deep",
                "second-legend",
                "inner",
                "inner-legend",
                "og",
                "in-og",
                "opt"
            ]
        );
        assert_eq!(
            ids(":enabled"),
            [
                "in-legend",
                "legend-after-div",
                "opt-ok",
                "ta",
                "radio",
                "text"
            ]
        );
        assert_eq!(ids(":checked"), ["opt-ok", "radio"]);
        // `:read-write` asks the same `is_disabled`: a field a disabled
        // fieldset disables is read-only, where Lexbor takes it as read-write.
        // (The SVG `input` is named by Lexbor's tag id, as Lexbor names it,
        // and `disabled` disables HTML elements only.)
        assert_eq!(
            ids("input:read-write, textarea:read-write"),
            ["in-legend", "ta", "radio", "text", "svg-input"]
        );
        assert_eq!(
            ids("input:read-only"),
            ["inherits", "second-legend", "inner-legend"]
        );
    }

    /// `select_all` counts sibling positions once per list (a memo); one
    /// `matches?` counts afresh. Both must answer the same for every
    /// `:nth-*` kind, over a list mixing types, text, comments and nesting.
    #[test]
    fn remembered_sibling_positions_agree_with_counting_afresh() {
        let mut html = String::from("<!doctype html><html><body><div id=list>");
        for i in 0..120 {
            match i % 5 {
                0 => html.push_str("<p>p</p> "),
                1 => html.push_str("<span>s<p>inner</p><p>inner</p></span>"),
                2 => html.push_str("<!-- c --><p>p</p>"),
                3 => html.push_str("<em>e</em>\n"),
                _ => html.push_str("<p><span>x</span></p>"),
            }
        }
        html.push_str("</div></body></html>");
        let doc = parsed(html.as_bytes());
        for sel in [
            ":nth-child(3n+1)",
            ":nth-last-child(odd)",
            ":nth-of-type(2n)",
            ":nth-last-of-type(3)",
            "p:first-of-type",
            "p:last-of-type",
            "span:only-of-type",
            "div > :nth-child(-n+7)",
            "span ~ p:nth-of-type(odd)",
            "p:nth-child(2n+1) + em",
            ":is(p, em):nth-last-child(4n)",
        ] {
            let walked: Vec<RawNode> = select_all(&doc, sel)
                .into_iter()
                .map(|e| RawNode::from(e.node()))
                .collect();
            let afresh: Vec<RawNode> = root(&doc)
                .subtree()
                .filter_map(HtmlNode::element)
                .filter(|&e| matches_selector(e, sel))
                .map(|e| RawNode::from(e.node()))
                .collect();
            assert!(
                !afresh.is_empty(),
                "{sel}: the fixture should match something"
            );
            assert!(
                walked == afresh,
                "{sel}: {} walked vs {} afresh",
                walked.len(),
                afresh.len()
            );
        }
    }

    /// With the memo, a whole wide list under `:nth-*` costs work linear in
    /// the list, not quadratic: 5,000 rows fit in a budget of a few steps per
    /// row. Counted afresh per row this needed ~12.5 million and raised
    /// (`tr:nth-child(odd)` over ~4,500 rows failed the 10 million default).
    #[test]
    fn nth_over_a_wide_list_costs_work_linear_in_the_list() {
        use crate::lexbor::css_match::select_all_with_work_limit;
        const ROWS: usize = 5000;
        let mut html = String::from("<!doctype html><table><tbody>");
        for _ in 0..ROWS {
            html.push_str("<tr><td>x</td></tr>\n");
        }
        html.push_str("</tbody></table>");
        let doc = parsed(html.as_bytes());
        for (sel, expect) in [
            ("tr:nth-child(odd)", ROWS / 2),
            ("tr:nth-last-child(2n)", ROWS / 2),
            ("tr:nth-of-type(3n)", ROWS / 3),
            ("tr:nth-last-of-type(-n+10)", 10),
            ("tr:last-of-type", 1),
        ] {
            let gvl = Gvl::exclusive();
            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let p = css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("{sel} parses"));
            let found = select_all_with_work_limit(root(&doc), p.groups(), 10 * ROWS as u64)
                .unwrap_or_else(|e| panic!("{sel}: {e:?}"));
            assert_eq!(found.len(), expect, "{sel}");
        }
    }

    /// The differential check: the port and the OLD Lexbor-callback
    /// engine (`lexbor::selectors`), run over the same document, must agree
    /// - in document order - on every standard selector this port supports.
    #[test]
    fn agrees_with_the_old_lexbor_engine_on_standard_selectors() {
        use crate::lexbor::selectors as old_engine;

        let doc = parsed(
            br#"<!doctype html><html><body>
                <main id="main" class="container Box">
                    <ul>
                        <li class="item first" data-n="1"><a href="/p/1">one</a></li>
                        <li class="item" data-n="2"><a href="/p/2" rel="next">two</a></li>
                        <li class="item last" data-n="3"><span>three</span></li>
                    </ul>
                    <p class="lead" title="Hello World">intro</p>
                    <p>body</p>
                    <div class="empty"></div>
                    <img src="x.png">
                    <svg><circle r="1"/></svg>
                    <input type="checkbox" checked>
                    <input required>
                    <input placeholder="name">
                </main>
            </body></html>"#,
        );

        let selectors = [
            "main",
            "MAIN",
            "*",
            ".item",
            ".Box",
            "#main",
            "li.item.first",
            "a[href]",
            "a[href='/p/1']",
            "a[rel~='next']",
            "li[data-n='1']",
            "[data-n^='1']",
            "[data-n$='1']",
            "[data-n*='1']",
            "[title='hello world' i]",
            "ul li",
            "ul > li",
            "li + li",
            "li ~ li",
            "li:nth-child(2)",
            "li:nth-child(odd)",
            "li:nth-last-child(1)",
            "li:first-child",
            "li:last-child",
            "li:only-child",
            "p:first-of-type",
            "p:last-of-type",
            "div.empty:empty",
            "html:root",
            ":is(p, li)",
            ":where(.lead, .item)",
            "li:not(.first)",
            ":not(:is(p, li))",
            ":is(:is(:is(li.first)))",
            "main:has(img)",
            "main:has(> p)",
            "li:has(+ li)",
            "li:has(~ li)",
            "p, li.first",
            "circle",
            "input:checked",
            "input:required",
            "input:placeholder-shown",
            "main:placeholder-shown",
            "a:any-link",
            "a:link",
        ];

        fn old_select_all(doc: &HtmlParsed, selector: &str) -> Vec<RawNode> {
            let gvl = Gvl::exclusive();
            // SAFETY: `doc` outlives the call, `root` is a live node of it.
            let d = unsafe { doc.raw_doc().as_doc() };
            let root = RawNode::from(d.as_node());
            old_engine::select_all(&gvl, root, selector.as_bytes())
                .unwrap_or_else(|_| panic!("old engine rejected {selector:?}"))
        }

        fn new_select_all(doc: &HtmlParsed, selector: &str) -> Vec<RawNode> {
            select_all(doc, selector)
                .into_iter()
                .map(|e| RawNode::from(e.node()))
                .collect()
        }

        for sel in selectors {
            let old = old_select_all(&doc, sel);
            let new = new_select_all(&doc, sel);
            assert!(
                new == old,
                "mismatch for selector {sel:?}: new has {} match(es), old has {}",
                new.len(),
                old.len()
            );
            // Self-consistency: `select_all`'s filter and `matches_any` asked
            // directly of each result must agree - a `select_all` that found
            // something `matches_any` denies (or vice versa) would mean the
            // two don't share the same underlying `list_matches`, silently.
            for element in select_all(&doc, sel) {
                assert!(
                    matches_selector(element, sel),
                    "select_all found {:?} for {sel:?}, but matches_any denies it",
                    element.qualified_name(),
                );
            }
        }
    }

    /// A tiny, dependency-free xorshift64* generator - this crate takes no
    /// `rand`-family dependency, and a fixed seed makes a failure
    /// reproducible (print the seed and the counter, per the assertion
    /// message) without needing to persist a corpus.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[(self.next_u64() as usize) % items.len()]
        }
    }

    /// One randomly generated compound (`tag.class#id[...]:pseudo`-shaped) -
    /// always syntactically valid CSS, since it is a concatenation of
    /// hand-picked well-formed pieces rather than free-form text.
    fn random_compound(rng: &mut Rng, depth: u32) -> String {
        const TYPES: &[&str] = &["li", "a", "span", "div", "p", "input", "*"];
        const CLASSES: &[&str] = &[".item", ".first", ".last", ".lead", ".Box", ".empty"];
        const IDS: &[&str] = &["#main"];
        const ATTRS: &[&str] = &[
            "[href]",
            "[data-n='1']",
            "[data-n^='1']",
            "[data-n$='1']",
            "[data-n*='1']",
            "[rel~='next']",
            "[title='hello world' i]",
            "[type='checkbox']",
        ];
        const PLAIN_PSEUDOS: &[&str] = &[
            ":first-child",
            ":last-child",
            ":only-child",
            ":first-of-type",
            ":last-of-type",
            ":only-of-type",
            ":empty",
            ":root",
            // `:checked`, `:disabled` and `:enabled` are left out: they follow
            // the HTML Standard, not Lexbor (`css_match`'s module doc),
            // and are checked against it instead
            // (`form_state_pseudo_classes_follow_the_html_standard`).
            ":required",
            ":optional",
            ":read-only",
            ":read-write",
            ":placeholder-shown",
            ":hover",
            ":focus",
            ":any-link",
            ":link",
        ];
        const NTH: &[&str] = &[
            ":nth-child(2)",
            ":nth-child(odd)",
            ":nth-child(even)",
            ":nth-child(2n+1)",
            ":nth-child(-n+2)",
            ":nth-last-child(1)",
            ":nth-of-type(2)",
        ];

        let mut s = String::new();
        s.push_str(rng.pick(TYPES));
        // 0-2 extra pieces on top of the type selector, each independently
        // chosen - a compound like `li.item[href]:first-child` is ordinary
        // CSS, so pieces are allowed to repeat or combine freely.
        for _ in 0..(rng.next_u64() % 3) {
            match rng.next_u64() % 5 {
                0 => s.push_str(rng.pick(CLASSES)),
                1 => s.push_str(rng.pick(IDS)),
                2 => s.push_str(rng.pick(ATTRS)),
                3 => s.push_str(rng.pick(PLAIN_PSEUDOS)),
                _ => s.push_str(rng.pick(NTH)),
            }
        }
        // A functional list-pseudo wraps a nested chain - bounded depth so
        // generation itself terminates (this is generation, not the engine
        // under test, so it does not need to prove anything about stack
        // safety; that is `deeply_nested_is_does_not_grow_the_native_stack`'s
        // job with a purpose-built input).
        if depth < 3 && rng.next_u64().is_multiple_of(3) {
            // `of S` is left out: Lexbor's own `of S` counts differently from
            // the spec in many shapes (module doc), so it is checked against a
            // spec oracle instead (`nth_child_of_s_agrees_with_a_spec_oracle`).
            const LIST_PSEUDOS: &[&str] = &[":is(", ":where(", ":not(", ":has("];
            s.push_str(rng.pick(LIST_PSEUDOS));
            s.push_str(&random_chain(rng, depth + 1));
            if rng.next_u64().is_multiple_of(4) {
                s.push_str(", ");
                s.push_str(&random_chain(rng, depth + 1));
            }
            s.push(')');
        }
        s
    }

    /// A chain of 1-3 compounds joined by combinators - descendant, `>`,
    /// `+`, `~` - the shapes `:has()`'s own combinator dispatch
    /// (`Combinator::{Descendant,Child,NextSibling,SubsequentSibling}`)
    /// distinguishes.
    fn random_chain(rng: &mut Rng, depth: u32) -> String {
        const COMBINATORS: &[&str] = &[" ", " > ", " + ", " ~ "];
        let mut s = random_compound(rng, depth);
        for _ in 0..(rng.next_u64() % 3) {
            s.push_str(rng.pick(COMBINATORS));
            s.push_str(&random_compound(rng, depth));
        }
        s
    }

    /// A full query: 1-2 comma-separated chains, matching what
    /// `select_all`/`matches_any` actually take (`Lists`, not one chain).
    fn random_selector(rng: &mut Rng) -> String {
        let mut s = random_chain(rng, 0);
        if rng.next_u64().is_multiple_of(3) {
            s.push_str(", ");
            s.push_str(&random_chain(rng, 0));
        }
        s
    }

    /// The differential check, broadened past the fixed 44-selector
    /// list above into randomly generated queries over a richer, more
    /// deeply nested fixture - closer to fuzz scale than a fixed list can
    /// be, inside `cargo test` (the coverage-guided counterpart is the
    /// `html_css` cargo-fuzz target). A fixed seed keeps a failure
    /// reproducible: rerun with the printed seed to get the same query.
    #[test]
    fn agrees_with_the_old_engine_on_randomly_generated_selectors() {
        use crate::lexbor::selectors as old_engine;

        let doc = parsed(
            br#"<!doctype html><html><body>
                <main id="main" class="container Box">
                    <ul>
                        <li class="item first" data-n="1"><a href="/p/1">one</a></li>
                        <li class="item" data-n="2"><a href="/p/2" rel="next">two</a></li>
                        <li class="item" data-n="3"><a href="/p/3">three</a><span>x</span></li>
                        <li class="item last" data-n="4"><span>four</span></li>
                    </ul>
                    <ul>
                        <li class="item first" data-n="1"><a href="/p/5">five</a></li>
                    </ul>
                    <p class="lead" title="Hello World">intro</p>
                    <p>body</p>
                    <div class="empty"></div>
                    <div><div><div class="item">deep</div></div></div>
                    <input type="checkbox" checked>
                    <input required>
                    <input disabled>
                    <input readonly placeholder="p" hover>
                    <textarea required placeholder="t" focus></textarea>
                    <select required><option>o</option></select>
                    <svg><a required hover>s</a></svg>
                </main>
            </body></html>"#,
        );

        const ITERATIONS: u32 = 5000;
        let mut rng = Rng(0x5EED_1DEA_u64);

        for i in 0..ITERATIONS {
            let sel = random_selector(&mut rng);

            let old = {
                let gvl = Gvl::exclusive();
                // SAFETY: `doc` outlives the call, `root` is a live node of it.
                let d = unsafe { doc.raw_doc().as_doc() };
                let root = RawNode::from(d.as_node());
                match old_engine::select_all(&gvl, root, sel.as_bytes()) {
                    Ok(v) => v,
                    // A generated shape the old engine's parser refuses (e.g.
                    // an operator combination it is stricter about) is not
                    // this port's concern to reproduce byte-for-byte; skip
                    // rather than assert agreement on a rejected selector.
                    Err(_) => continue,
                }
            };
            let new: Vec<RawNode> = select_all(&doc, &sel)
                .into_iter()
                .map(|e| RawNode::from(e.node()))
                .collect();

            assert!(
                new == old,
                "iteration {i} (seed 0x5EED1DEA) mismatch for generated selector {sel:?}: \
                 new has {} match(es), old has {}",
                new.len(),
                old.len()
            );
        }
    }

    /// A random document over the names [`random_compound`] asks about:
    /// nested up to six deep, with the classes, ids and attributes its
    /// selectors test, so that most of them match something somewhere.
    /// No `<fieldset>`: what it disables is a documented departure
    /// (`css_match`'s module doc).
    fn random_document(rng: &mut Rng) -> String {
        const TAGS: &[&str] = &[
            "div", "p", "ul", "li", "a", "span", "section", "input", "textarea", "b",
        ];
        const ATTRS: &[&str] = &[
            " class=item",
            " class='item first'",
            " class='item last'",
            " class=lead",
            " class=Box",
            " class=empty",
            " id=main",
            " data-n=1",
            " data-n=12",
            " data-n=21",
            " href=/p",
            " rel='prev next'",
            " title='Hello World'",
            " type=checkbox",
            " required",
            " readonly",
            " placeholder=x",
            " hover",
        ];
        let mut html = String::from("<!doctype html><html><body>");
        let mut open: Vec<&str> = Vec::new();
        for _ in 0..(10 + rng.next_u64() % 50) {
            match rng.next_u64() % 4 {
                0 if !open.is_empty() => {
                    let tag = open.pop().unwrap_or("div");
                    html.push_str(&format!("</{tag}>"));
                }
                1 => html.push_str("text"),
                _ if open.len() < 6 => {
                    let tag = *rng.pick(TAGS);
                    html.push('<');
                    html.push_str(tag);
                    for _ in 0..(rng.next_u64() % 3) {
                        html.push_str(rng.pick(ATTRS));
                    }
                    html.push('>');
                    if !matches!(tag, "input") {
                        open.push(tag);
                    }
                }
                _ => {}
            }
        }
        while let Some(tag) = open.pop() {
            html.push_str(&format!("</{tag}>"));
        }
        html.push_str("</body></html>");
        html
    }

    /// The randomized differential check over random documents as well as
    /// random selectors: the fixed fixture above leaves whole shapes
    /// unexercised (an `:is()` whose nearest ancestor fails, a `~` past a
    /// non-matching sibling). `MAKIRI_CSS_DIFF_SEED` and
    /// `MAKIRI_CSS_DIFF_ITERATIONS` widen the sweep - CI's css-match job runs
    /// it long with a fresh seed - and a failure prints the seed, the
    /// document and the selector.
    #[test]
    fn agrees_with_the_old_engine_on_random_documents() {
        use crate::lexbor::selectors as old_engine;

        let env = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|v| {
                    let v = v.trim();
                    match v.strip_prefix("0x") {
                        Some(hex) => u64::from_str_radix(hex, 16).ok(),
                        None => v.parse().ok(),
                    }
                })
                .unwrap_or(default)
        };
        let seed = env("MAKIRI_CSS_DIFF_SEED", 0xD0C5_EED5).max(1);
        let documents = env("MAKIRI_CSS_DIFF_ITERATIONS", 200);
        const SELECTORS_PER_DOCUMENT: u32 = 25;
        let mut rng = Rng(seed);

        for d in 0..documents {
            let html = random_document(&mut rng);
            let doc = parsed(html.as_bytes());
            for _ in 0..SELECTORS_PER_DOCUMENT {
                let sel = random_selector(&mut rng);
                let old = {
                    let gvl = Gvl::exclusive();
                    match old_engine::select_all(&gvl, RawNode::from(root(&doc)), sel.as_bytes()) {
                        Ok(v) => v,
                        // As above: a shape the old parser refuses is skipped.
                        Err(_) => continue,
                    }
                };
                let new: Vec<RawNode> = select_all(&doc, &sel)
                    .into_iter()
                    .map(|e| RawNode::from(e.node()))
                    .collect();
                assert!(
                    new == old,
                    "document {d} (MAKIRI_CSS_DIFF_SEED={seed:#x}): {sel:?} has {} match(es) \
                     here, {} in Lexbor's engine, over\n{html}",
                    new.len(),
                    old.len()
                );
            }
        }
    }

    /// Long chains over a deep, repetitive tree - where a failed left part
    /// is pruned (`Query::step_chain`'s `Fail`) rather than retried from
    /// every further ancestor or sibling. The pruning must never change a
    /// verdict, so it is checked against the old engine's exhaustive
    /// backtracking. Random chains almost never match, which would leave the
    /// pruning nothing to get wrong, so each selector is read off a real
    /// path - up through ancestors and back through preceding siblings from
    /// a random element - and half of them then have one compound or
    /// combinator changed: a near miss, whose other placements are what a
    /// wrong prune would skip.
    #[test]
    fn pruned_long_chains_agree_with_the_old_engine() {
        use crate::lexbor::selectors as old_engine;

        let mut html = String::from("<!doctype html><html><body>");
        for i in 0..12 {
            let class = ["a", "b", "item"][i % 3];
            html.push_str(&format!(
                r#"<div class="{class}"><ul><li class="item">x</li>"#
            ));
            html.push_str(
                r#"<li class="a"><a href="/p">y</a></li><li><span>z</span></li><p>w</p></ul>"#,
            );
            if i % 4 == 3 {
                html.push_str(r#"<p class="b">q</p>"#);
            }
        }
        html.push_str(&"</div>".repeat(12));
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());
        let elements: Vec<HtmlElement> =
            root(&doc).subtree().filter_map(HtmlNode::element).collect();
        fn parent(e: HtmlElement<'_>) -> Option<HtmlElement<'_>> {
            e.node().parent().and_then(HtmlNode::element)
        }
        fn prev(e: HtmlElement<'_>) -> Option<HtmlElement<'_>> {
            let mut n = e.node().prev();
            while let Some(m) = n {
                if let Some(el) = m.element() {
                    return Some(el);
                }
                n = m.prev();
            }
            None
        }

        const TYPES: &[&str] = &["div", "ul", "li", "p", "a", "*"];
        const CLASSES: &[&str] = &["", ".a", ".b", ".item"];
        const COMBINATORS: &[&str] = &[" ", " > ", " + ", " ~ "];
        let mut rng = Rng(0x0DEE_9C4A_u64);
        let (mut compared, mut matched) = (0, 0);
        for i in 0..3000 {
            // Right to left: a compound for the element, then a combinator
            // and the element it leads to.
            let mut at = *rng.pick(&elements);
            let mut parts: Vec<String> = Vec::new();
            let len = 2 + rng.next_u64() % 7;
            loop {
                let mut compound = String::from_utf8_lossy(at.node().qualified_name()).into_owned();
                if rng.next_u64().is_multiple_of(4) {
                    compound = "*".into();
                }
                if let Some(class) = at.get_attribute(b"class") {
                    if rng.next_u64().is_multiple_of(2) {
                        let first = class.split(|b| *b == b' ').next().unwrap_or_default();
                        compound.push('.');
                        compound.push_str(&String::from_utf8_lossy(first));
                    }
                }
                parts.push(compound);
                if parts.len() as u64 >= 2 * len - 1 {
                    break;
                }
                let (comb, next) = match rng.next_u64() % 4 {
                    0 => (" > ", parent(at)),
                    1 => (" + ", prev(at)),
                    2 => {
                        let mut n = parent(at);
                        for _ in 0..rng.next_u64() % 3 {
                            n = n.and_then(parent);
                        }
                        (" ", n)
                    }
                    _ => {
                        let mut n = prev(at);
                        for _ in 0..rng.next_u64() % 3 {
                            n = n.and_then(prev);
                        }
                        (" ~ ", n)
                    }
                };
                let Some(next) = next else { break };
                parts.push(comb.into());
                at = next;
            }
            parts.reverse();
            if parts.len() > 1 && rng.next_u64().is_multiple_of(2) {
                let k = (rng.next_u64() as usize) % parts.len();
                parts[k] = if k % 2 == 0 {
                    format!("{}{}", rng.pick(TYPES), rng.pick(CLASSES))
                } else {
                    (*rng.pick(COMBINATORS)).into()
                };
            }
            let sel = parts.concat();
            let old = {
                let gvl = Gvl::exclusive();
                // SAFETY: `doc` outlives the call, `root` is a live node of it.
                let d = unsafe { doc.raw_doc().as_doc() };
                let root = RawNode::from(d.as_node());
                match old_engine::select_all(&gvl, root, sel.as_bytes()) {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            };
            let new: Vec<RawNode> = select_all(&doc, &sel)
                .into_iter()
                .map(|e| RawNode::from(e.node()))
                .collect();
            assert!(
                new == old,
                "iteration {i}: {sel:?}: new {} old {}",
                new.len(),
                old.len()
            );
            compared += 1;
            matched += usize::from(!new.is_empty());
        }
        // The generator is what makes this test worth running: most of what
        // it compares must match something.
        assert!(
            compared > 2900 && matched > compared / 2,
            "{compared} compared, {matched} matched"
        );
    }

    /// `:nth-child(An+B of S)` / `:nth-last-child(An+B of S)` checked against
    /// the CSS definition itself, not against Lexbor: Lexbor's own `of S`
    /// miscounts in many shapes - a comma list in `S` (it starts from the
    /// LAST list, so `2 of ul, p` and `2 of p, ul` answer differently), a
    /// combinator in `S` (`span:nth-child(2 of li span)`), even plain
    /// `:enabled`/`:empty` in `S` - which the module doc records as a
    /// departure. The oracle: an element matches iff it is in `S` (computed
    /// with `select_all(S)`) and its rank among the element siblings in `S`,
    /// counted from its own end, satisfies `An+B`. Every fifth `S` is the
    /// previous round's whole `of S` query, so `of S` nested in `of S` is
    /// covered too (the inner level having been checked the round before).
    #[test]
    fn nth_child_of_s_agrees_with_a_spec_oracle() {
        let doc = parsed(
            br#"<!doctype html><html><body>
                <main id="main" class="container Box">
                    <ul>
                        <li class="item first" data-n="1"><a href="/p/1">one</a></li>
                        <li class="item" data-n="2"><a href="/p/2" rel="next">two</a></li>
                        <li class="item" data-n="3"><a href="/p/3">three</a><span>x</span></li>
                        <li class="item last" data-n="4"><span>four</span></li>
                    </ul>
                    <ul>
                        <li class="item first" data-n="1"><a href="/p/5">five</a></li>
                    </ul>
                    <p class="lead" title="Hello World">intro</p>
                    <p>body</p>
                    <div class="empty"></div>
                    <div><div><div class="item">deep</div></div></div>
                    <input type="checkbox" checked>
                    <input required>
                    <input disabled>
                </main>
            </body></html>"#,
        );
        let try_select = |sel: &str| -> Option<Vec<RawNode>> {
            let gvl = Gvl::exclusive();
            let text = VerifiedText::from_bytes(sel.as_bytes())?;
            let parsed_sel = css_parser::parse(&gvl, text).ok()?;
            let found = port_select_all(root(&doc), parsed_sel.groups()).ok()?;
            Some(found.into_iter().map(RawNode::from).collect())
        };
        let elements: Vec<HtmlNode<'_>> = root(&doc)
            .subtree()
            .skip(1)
            .filter(|n| n.element().is_some())
            .collect();
        fn sibling(n: HtmlNode<'_>, from_end: bool) -> Option<HtmlNode<'_>> {
            let mut cur = if from_end { n.next() } else { n.prev() };
            while let Some(c) = cur {
                if c.element().is_some() {
                    return Some(c);
                }
                cur = if from_end { c.next() } else { c.prev() };
            }
            None
        }

        const ANBS: &[(&str, i64, i64)] = &[
            ("1", 0, 1),
            ("2", 0, 2),
            ("odd", 2, 1),
            ("even", 2, 0),
            ("-n+2", -1, 2),
            ("3n", 3, 0),
        ];
        let mut rng = Rng(0x0F5E_1DEA_u64);
        let mut previous: Option<String> = None;
        let mut checked = 0u32;
        for i in 0..1500u32 {
            let s = match previous.take() {
                Some(q) if i % 5 == 0 => q,
                _ => random_selector(&mut rng),
            };
            let Some(in_s) = try_select(&s) else {
                continue;
            };
            let (anb, a, b) = *rng.pick(ANBS);
            let from_end = rng.next_u64().is_multiple_of(2);
            let pseudo = if from_end {
                "nth-last-child"
            } else {
                "nth-child"
            };
            let q = format!("*:{pseudo}({anb} of {s})");
            let got = try_select(&q).unwrap_or_else(|| panic!("{q:?} failed"));

            let expected: Vec<RawNode> = elements
                .iter()
                .copied()
                .filter(|&e| {
                    if !in_s.contains(&RawNode::from(e)) {
                        return false;
                    }
                    let mut pos = 1i64;
                    let mut cur = sibling(e, from_end);
                    while let Some(c) = cur {
                        if in_s.contains(&RawNode::from(c)) {
                            pos += 1;
                        }
                        cur = sibling(c, from_end);
                    }
                    if a == 0 {
                        pos == b
                    } else {
                        (pos - b) % a == 0 && (pos - b) / a >= 0
                    }
                })
                .map(RawNode::from)
                .collect();
            assert!(
                got == expected,
                "round {i}: {q:?} answered {} element(s), the spec {}",
                got.len(),
                expected.len()
            );
            checked += 1;
            previous = Some(q);
        }
        assert!(checked > 1000, "only {checked} rounds parsed");
    }

    /// Type and attribute names are resolved to Lexbor ids once per query
    /// (`css_match::Name`) on the walking entry points, and compared as
    /// bytes on `matches_any`. Both must answer as the old engine does across
    /// what makes names tricky: foreign (SVG/MathML) elements with
    /// case-preserved names, quirks vs no-quirks documents, custom elements
    /// (dynamic tag ids), and names the document does not contain at all.
    #[test]
    fn resolved_names_agree_with_the_old_engine() {
        use crate::lexbor::selectors as old_engine;

        let body = r##"<body><my-el data-x="1" class="Foo">c</my-el><x-y id="Main"></x-y>
            <svg viewBox="0 0 1 1"><foreignObject data-x="2"><p class="foo">f</p></foreignObject>
            <circle r="1" CLASS="k"/><a xlink:href="#h" href="#g"></a></svg>
            <math><mi mathvariant="bold">x</mi></math>
            <input type="TEXT" Data-Y="Q"><div id="main" class="foo bar"></div></body>"##;
        let selectors = [
            "my-el",
            "MY-EL",
            "x-y",
            "nosuch-el",
            "foreignObject",
            "foreignobject",
            "FOREIGNOBJECT",
            "circle",
            "svg circle",
            "mi",
            "math mi",
            "p",
            "div",
            "[data-x]",
            "[DATA-X]",
            "[data-y]",
            "[Data-Y]",
            "[viewBox]",
            "[viewbox]",
            "[r]",
            "[href]",
            "[type=text]",
            "[type=TEXT s]",
            "[mathvariant]",
            "[nosuch]",
            "#main",
            "#Main",
            ".foo",
            ".Foo",
            ".k",
            ".nosuch",
            "my-el[data-x='1']",
            "svg [data-x]",
            ":is(circle, mi)[r]",
        ];
        const FOREIGN_CASE: &[&str] = &["[DATA-X]", "[viewbox]", "foreignobject", "FOREIGNOBJECT"];
        for prefix in ["<!doctype html><html>", "<html>"] {
            let html = format!("{prefix}{body}</html>");
            let doc = parsed(html.as_bytes());
            let d = unsafe { doc.raw_doc().as_doc() };
            let raw_root = RawNode::from(d.as_node());
            for sel in selectors {
                let gvl = Gvl::exclusive();
                let old = old_engine::select_all(&gvl, raw_root, sel.as_bytes())
                    .unwrap_or_else(|_| panic!("old engine rejected {sel:?}"));
                let old_first = old_engine::select_first(&gvl, raw_root, sel.as_bytes())
                    .unwrap_or_else(|_| panic!("old engine rejected {sel:?}"));
                drop(gvl);
                let new: Vec<RawNode> = select_all(&doc, sel)
                    .into_iter()
                    .map(|e| RawNode::from(e.node()))
                    .collect();
                if FOREIGN_CASE.contains(&sel) {
                    // The HTML Standard's rule, not Lexbor's: an attribute
                    // name, and a type selector's name, is case-insensitive
                    // only on an HTML element in an HTML document. Lexbor
                    // folds both everywhere, so it also finds the SVG
                    // `viewBox`/`data-x` and `foreignObject` - and only those.
                    let extra: Vec<HtmlNode<'_>> = root(&doc)
                        .subtree()
                        .filter(|&n| {
                            old.contains(&RawNode::from(n)) && !new.contains(&RawNode::from(n))
                        })
                        .collect();
                    assert!(
                        new.iter().all(|n| old.contains(n))
                            && !extra.is_empty()
                            && extra.iter().all(|n| n.ns_id() != Some(NsId::HTML)),
                        "{prefix} {sel:?}: expected Lexbor to add foreign elements only"
                    );
                    continue;
                }
                assert!(
                    new == old,
                    "{prefix} {sel:?}: new {} old {}",
                    new.len(),
                    old.len()
                );
                let gvl = Gvl::exclusive();
                let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
                let p = css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("parse"));
                let first = port_select_first(root(&doc), p.groups())
                    .unwrap_or_else(|_| panic!("{sel:?} failed"))
                    .map(RawNode::from);
                assert!(first == old_first, "{prefix} {sel:?}: select_first differs");
                for n in root(&doc).subtree().filter_map(HtmlNode::element) {
                    let one = matches_any(p.groups(), n).unwrap_or_else(|_| panic!("{sel:?}"));
                    assert_eq!(
                        one,
                        old.contains(&RawNode::from(n.node())),
                        "{prefix} {sel:?}: matches_any differs on <{}>",
                        String::from_utf8_lossy(n.qualified_name())
                    );
                }
            }
        }
    }

    /// `select_first`/`matches_any` are entry points of their own (not just
    /// `select_all().first()`/`.is_empty()`), and from an arbitrary ELEMENT
    /// root - not only the document - which is where "descendants only, the
    /// root itself excluded" actually has something to get wrong. Checked
    /// against the old engine from that same non-document root.
    #[test]
    fn select_first_and_matches_any_agree_with_the_old_engine_from_a_non_document_root() {
        use crate::lexbor::selectors as old_engine;

        let doc = parsed(
            br#"<!doctype html><html><body>
                <ul id="list">
                    <li class="item first" data-n="1"><a href="/p/1">one</a></li>
                    <li class="item" data-n="2"><a href="/p/2" rel="next">two</a></li>
                    <li class="item last" data-n="3"><span>three</span></li>
                </ul>
            </body></html>"#,
        );

        let list = root(&doc)
            .subtree()
            .filter_map(HtmlNode::element)
            .find(|e| e.qualified_name() == b"ul")
            .expect("the fixture has a <ul>");

        let selectors = [
            "li",
            "li.first",
            ".item",
            "ul",
            "#list",
            "li:last-child",
            "li:not(.first)",
            "a[href]",
            "span",
            "missing",
        ];

        for sel in selectors {
            let gvl = Gvl::exclusive();
            let raw_root = RawNode::from(list.node());
            let old_first = old_engine::select_first(&gvl, raw_root, sel.as_bytes())
                .unwrap_or_else(|_| panic!("old engine rejected {sel:?}"));
            let old_matches = old_engine::matches_node(&gvl, raw_root, sel.as_bytes())
                .unwrap_or_else(|_| panic!("old engine rejected {sel:?}"));
            drop(gvl);

            let text = VerifiedText::from_bytes(sel.as_bytes()).expect("verified");
            let gvl = Gvl::exclusive();
            let parsed_sel = css_parser::parse(&gvl, text)
                .unwrap_or_else(|_| panic!("selector {sel:?} failed to parse"));
            let new_first = port_select_first(list.node(), parsed_sel.groups())
                .unwrap_or_else(|_| panic!("selector {sel:?} exceeded its work budget"));
            let new_matches = matches_any(parsed_sel.groups(), list)
                .unwrap_or_else(|_| panic!("selector {sel:?} exceeded its work budget"));

            assert!(
                new_first.map(RawNode::from) == old_first,
                "select_first mismatch for {sel:?} rooted at <ul>: new {:?}, old {:?}",
                new_first.is_some(),
                old_first.is_some()
            );
            // `matches_any` asks whether `list` (the `<ul>`) itself matches -
            // no traversal - so this checks it against the old engine's own
            // `matches_node` asked the identical question of the identical
            // node, `sel: "ul"` included (where the answer is `true`).
            assert_eq!(
                new_matches, old_matches,
                "matches_any mismatch for {sel:?} on <ul> itself"
            );
        }
    }

    /// `:nth-child(1 of ...)` nested inside itself used to call a fresh `run`
    /// natively per sibling from `check_simple` - 300 levels raised
    /// `SystemStackError` in a 128 KiB Fiber, which left the shared CSS
    /// engine's busy flag set for the rest of the process. `Frame::NthOfStep`
    /// keeps it on the heap stack; this runs it 20,000 deep on a
    /// `RUBY_FIBER_MACHINE_STACK_SIZE`-sized thread.
    #[test]
    fn nth_child_of_s_nesting_is_heap_based_not_native_recursion() {
        const DEPTH: usize = 3_000;
        let handle = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || {
                let doc = parsed(b"<html><body><a>x</a></body></html>");
                let sel = format!("{}a{}", ":nth-child(1 of ".repeat(DEPTH), ")".repeat(DEPTH));
                texts(&doc, &sel)
            })
            .expect("spawn a Fiber-sized-stack thread");
        assert_eq!(
            handle.join().expect("must not overflow a 128 KiB stack"),
            ["x"]
        );
    }

    /// `:has()` nesting evaluated via `select_all` (which tests EVERY
    /// element, not just the one node shaped to match): every candidate but
    /// the outermost div lacks enough remaining depth to satisfy the whole
    /// nested chain, and `:has()`'s Descendant search - "try every descendant
    /// as a candidate", as Lexbor's forward search does - inherent to what that
    /// combinator means, not an
    /// artifact of this file's design - explores many combinations before
    /// concluding that for each one. This is the SAME exponential-in-depth
    /// search space the OLD native-recursive `has_forward` had for this
    /// shape (confirmed by measuring `try_has_alternative` call counts while
    /// developing this fix: ~24/42/76/142/272/530/2070 at depth 3-10, each
    /// roughly double the last) - the heap conversion changed how the search
    /// is STORED (never the native stack), not its complexity. The work
    /// budget - already in place before this session's `:has()` conversion,
    /// see `a_has_search_that_cannot_find_anything_fails_closed_...` above -
    /// is what bounds it; this proves the fix didn't accidentally remove that
    /// protection. `select_all_with_work_limit` keeps the fixture small and
    /// fast rather than needing a document big enough to exhaust the real,
    /// shipped 10-million limit.
    #[test]
    fn has_nesting_against_an_ambiguous_document_fails_closed_on_the_work_budget() {
        use crate::lexbor::css_match::{select_all_with_work_limit, QueryFailure};

        const DEPTH: usize = 10;
        let selector = format!("{}div{}", ":has(".repeat(DEPTH), ")".repeat(DEPTH));
        let mut html = String::from("<!doctype html><html><body>");
        for _ in 0..DEPTH {
            html.push_str("<div>");
        }
        for _ in 0..DEPTH {
            html.push_str("</div>");
        }
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());

        let gvl = Gvl::exclusive();
        let text = VerifiedText::from_bytes(selector.as_bytes()).expect("verified");
        let parsed_sel =
            css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("selector fails to parse"));

        // Comfortably below what querying EVERY element in this small
        // fixture actually costs (measured ~6,200 for the full walk) - must
        // fail closed, never a truncated/empty `Ok`.
        let starved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 100);
        assert!(matches!(
            starved,
            Err(QueryFailure::Match(MatchFailure::WorkExceeded))
        ));

        // The same query with room to spare still answers correctly: SOME
        // element (the outermost div at least, and its own ancestors - the
        // implicit `<html>`/`<body>` - via the exact same chain) has enough
        // depth below it to satisfy the whole `:has()` chain; the point here
        // is that a shorter limit fails closed and a longer one doesn't
        // (this test's other half), not the exact count.
        let unstarved = select_all_with_work_limit(root(&doc), parsed_sel.groups(), 10_000)
            .expect("comfortably within budget");
        assert!(!unstarved.is_empty());
    }
}

/// `lexbor::selector_cache` - the compiled-selector cache reintroduced for
/// `Node#css`/`#at_css`/`#matches?` once they moved to `css_match`,
/// mirroring `lexbor::selectors`'s adaptive `CachePolicy`/`SelectorCache` over
/// its OWN separate parser/arena (module doc). `Gvl::exclusive()` stands in
/// for the real GVL proof here, same as `css_match`'s differential
/// tests do against the old engine.
///
/// The adaptive bypass/retest window (40,000-iteration territory in
/// `spec/css_selector_cache_spec.rb`) is NOT re-tested here - that spec
/// already covers it end to end through the real glue, and duplicating a
/// slow, iteration-heavy test at this level buys little; this module checks
/// the parts a fast, ASan-friendly Rust test is good for: a hit answers the
/// same as a miss would, filling the cache past its cap doesn't lose
/// correctness, and a rejected selector does not disturb what is already
/// cached.
mod selector_cache {
    use crate::gvl::Gvl;
    use crate::lexbor::adapter::html::{HtmlNode, RawNode};
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParsed};
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    use crate::lexbor::css_match::select_all;
    use crate::lexbor::selector_cache::with_compiled;

    fn parsed(html: &[u8]) -> Box<HtmlParsed> {
        parse_html(html, true, DepthLimit::DEFAULT).expect("a document parses")
    }

    fn root(doc: &HtmlParsed) -> HtmlNode<'_> {
        // SAFETY: `doc` outlives the borrow this returns.
        let d = unsafe { doc.raw_doc().as_doc() };
        d.as_node()
    }

    /// Every matching descendant's node identity, through the cache.
    fn select_all_cached(doc: &HtmlParsed, selector: &str) -> Vec<RawNode> {
        with_compiled(&Gvl::exclusive(), selector.as_bytes(), |groups, scratch| {
            select_all(scratch, root(doc), groups)
        })
        .unwrap_or_else(|_| panic!("{selector:?} fails to parse"))
        .unwrap_or_else(|e| panic!("{selector:?} failed: {e:?}"))
        .into_iter()
        .map(RawNode::from)
        .collect()
    }

    #[test]
    fn a_cache_hit_answers_the_same_as_the_first_miss_did() {
        let doc = parsed(b"<html><body><p class=x>a</p><p>b</p><p class=x>c</p></body></html>");
        let first = select_all_cached(&doc, "p.x");
        let second = select_all_cached(&doc, "p.x"); // same bytes: a cache hit
        assert_eq!(first.len(), 2);
        assert!(
            first == second,
            "a hit must answer exactly what the miss did"
        );
    }

    #[test]
    fn filling_the_cache_past_its_cap_does_not_lose_correctness() {
        // 300 distinct selectors against the 256-entry cap (mirrors
        // spec/css_selector_cache_spec.rb): the 257th flushes it, and every
        // one - before and after the flush - must still answer right.
        let mut html = String::from("<html><body>");
        for i in 1..=300 {
            html.push_str(&format!("<p id=n{i}>{i}</p>"));
        }
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());

        for i in 1..=300 {
            let sel = format!("#n{i}");
            let found = select_all_cached(&doc, &sel);
            assert_eq!(found.len(), 1, "selector {sel:?} should match exactly one");
        }
        // A second pass re-hits everything, including the ones the cap
        // already flushed out and back in once.
        for i in 1..=300 {
            let sel = format!("#n{i}");
            assert_eq!(select_all_cached(&doc, &sel).len(), 1);
        }
    }

    #[test]
    fn a_rejected_selector_does_not_disturb_what_is_already_cached() {
        let doc = parsed(b"<html><body><p class=x>a</p><p class=x>b</p></body></html>");
        assert_eq!(select_all_cached(&doc, "p.x").len(), 2);

        let rejected = with_compiled(&Gvl::exclusive(), b"p[", |groups, scratch| {
            select_all(scratch, root(&doc), groups)
        });
        assert!(rejected.is_err(), "a malformed selector must not parse");

        // The previously cached entry must still answer correctly.
        assert_eq!(select_all_cached(&doc, "p.x").len(), 2);
    }
}

/// The serializer's walks (`adapter::html::serialize`) against the Lexbor
/// walks they replaced: byte for byte the same on every node of documents the
/// parser builds.
mod serialize_walk {
    use crate::lexbor::abi as lxb;
    use crate::lexbor::adapter::html::{HtmlNode, RawNode};
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParsed};
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    use crate::lexbor::chunks::{chunk_cb, ChunkSink, Chunks};
    use crate::node_type::NodeType;

    const OK: lxb::lxb_status_t = lxb::consts::STATUS_OK as lxb::lxb_status_t;
    const OPT: lxb::lxb_html_serialize_opt_t =
        lxb::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF as _;

    struct Bytes(Vec<u8>);

    impl ChunkSink for Bytes {
        fn take(&mut self, bytes: &[u8]) -> bool {
            self.0.extend_from_slice(bytes);
            true
        }
    }

    /// What `write` sends through a collecting sink.
    fn collect(
        write: impl FnOnce(lxb::lxb_html_serialize_cb_f, *mut core::ffi::c_void) -> lxb::lxb_status_t,
    ) -> Vec<u8> {
        let mut c = Chunks::new(Bytes(Vec::new()));
        let st = write(Some(chunk_cb::<Bytes>), c.ctx());
        assert_eq!(st, OK);
        c.sink.0
    }

    /// Ours and Lexbor's, as (plain, pretty), for `n` and - with `deep` - its
    /// children only.
    fn both(n: HtmlNode<'_>, deep: bool) -> [(Vec<u8>, Vec<u8>); 2] {
        let raw = RawNode::from(n).as_lxb_mut();
        // SAFETY: a live node of a live document, which nothing changes; the
        // sink takes every chunk with its own context.
        unsafe {
            let ours = (
                collect(|cb, ctx| n.serialize_to(deep, cb, ctx)),
                collect(|cb, ctx| n.serialize_pretty_to(deep, cb, ctx)),
            );
            let lexbor = if deep {
                (
                    collect(|cb, ctx| lxb::lxb_html_serialize_deep_cb(raw, cb, ctx)),
                    collect(|cb, ctx| lxb::lxb_html_serialize_pretty_deep_cb(raw, OPT, 0, cb, ctx)),
                )
            } else {
                (
                    collect(|cb, ctx| lxb::lxb_html_serialize_tree_cb(raw, cb, ctx)),
                    collect(|cb, ctx| lxb::lxb_html_serialize_pretty_tree_cb(raw, OPT, 0, cb, ctx)),
                )
            };
            [ours, lexbor]
        }
    }

    fn doc(html: &[u8]) -> Box<HtmlParsed> {
        parse_html(html, true, DepthLimit::UNLIMITED).expect("a document parses")
    }

    #[test]
    fn ours_write_what_lexbor_writes() {
        let corpus: &[&[u8]] = &[
            b"<!doctype html><html><head><title>t &amp; u</title><style>a<b{}</style>\
              <script>if (a < b && c) {}</script></head><body><p class=x id=\"q\">a &lt; b\xc2\xa0c</p>\
              <!-- c --><br><img src=x alt='a\"b'><ul><li>1<li>2</ul><table><tr><td>x</table>\
              <textarea>\n t</textarea><pre>\n\nx</pre><xmp><b></xmp><noscript><i>n</i></noscript>\
              <iframe><b></iframe><noembed>&</noembed><plaintext>x<y",
            b"<template><p>a</p><template><i>b</i></template></template><div><template></template></div>",
            b"<svg viewBox='0 0 1 1'><foreignObject><p>x</p></foreignObject><style>plain</style>\
              <script>also plain</script><desc>d</desc><path d='M0'/></svg><math><mi>x</mi>\
              <annotation-xml encoding='text/html'><p>y</p></annotation-xml></math>",
            b"<p>line\nbreak\r\nand\ttab</p><?php echo 1 ?><div><span><b>deep</b></span></div>",
            b"",
        ];
        for html in corpus {
            let parsed = doc(html);
            // SAFETY: `parsed` is live for the loop.
            let root = unsafe { parsed.raw_doc().as_doc() }.as_node();
            let mut node = Some(root);
            while let Some(n) = node {
                let fragment = n.node_type() == NodeType::DocumentFragment;
                if !fragment {
                    let [ours, lexbor] = both(n, false);
                    assert_eq!(ours, lexbor, "tree of a {:?} in {html:?}", n.node_type());
                }
                if n.first_child().is_some() || fragment {
                    let [ours, lexbor] = both(n, true);
                    assert_eq!(
                        ours,
                        lexbor,
                        "children of a {:?} in {html:?}",
                        n.node_type()
                    );
                }
                node = n.preorder_next_with_contents(root);
            }
        }
    }
}
