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

/// `lexbor::selector_port` - a semantic port of Lexbor's `selectors.c`
/// (`notes/lexbor_selectors_c_semantics.ja.md`), not a `selectors`-crate-based
/// approach (rejected - see the plan's §1.1). Parses via the existing `css_parser`, matches via an
/// explicit heap work stack (never native recursion for selector nesting -
/// verified at 500,000 levels below).
mod selector_port_spike {
    use crate::gvl::Gvl;
    use crate::lexbor::adapter::html::{HtmlElement, HtmlNode, RawNode};
    use crate::lexbor::adapter::post_parse::{parse_html, HtmlParsed};
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    use crate::lexbor::css_parser;
    use crate::lexbor::selector_port::{
        matches_any, select_all as port_select_all, select_first as port_select_first,
        MAX_COMPOUNDS,
    };
    use crate::text::VerifiedText;

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
            .unwrap_or_else(|_| panic!("selector {selector:?} overflowed NODE_SET_MAX"))
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

    #[test]
    fn type_selector_folds_ascii_case_unconditionally() {
        // §B-1: Lexbor folds type-selector case UNCONDITIONALLY - even on
        // foreign (SVG) content, unlike class/id (quirks-only) or attributes
        // (HTML-namespace-gated). Reproduced for engine parity.
        let doc = parsed(b"<html><body><p>1</p><span>2</span><p>3</p></body></html>");
        assert_eq!(texts(&doc, "p"), ["1", "3"]);
        assert_eq!(texts(&doc, "P"), ["1", "3"]);

        let svg = parsed(b"<html><body><svg><circle r='1'/></svg></body></html>");
        assert_eq!(select_all(&svg, "CIRCLE").len(), 1);
    }

    #[test]
    fn class_and_id_fold_case_only_in_quirks_mode() {
        // §B-2/§B-3, §E-1 point 5 (`match_id_class_case`).
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
        // §B-5, §E-1 point 9 (`match_html_case_insensitive_attributes`):
        // `type` is in the table (HTML, no modifier -> CI); `data-x` is not
        // (always CS); `s` forces case-sensitive even for a table attribute;
        // the table does not apply inside SVG (foreign content).
        let doc = parsed(
            b"<!doctype html><html><body>\
              <input type='TEXT'><div data-x='ABC'></div>\
              <svg><a rel='NOFOLLOW'></a></svg>\
              </body></html>",
        );
        assert_eq!(select_all(&doc, "[type=TEXT]").len(), 1);
        assert_eq!(select_all(&doc, "[type=text]").len(), 1); // table default: CI
        assert_eq!(select_all(&doc, "[type=text s]").len(), 0); // explicit s forces CS
        assert_eq!(select_all(&doc, "[data-x=abc]").len(), 0); // not in the table: CS
        assert_eq!(select_all(&doc, "[data-x=ABC]").len(), 1);
        assert_eq!(select_all(&doc, "[rel=nofollow]").len(), 0); // SVG: table doesn't apply
        assert_eq!(select_all(&doc, "[rel=NOFOLLOW]").len(), 1);
    }

    #[test]
    fn lexbor_whitespace_set_excludes_vertical_tab() {
        // §E-2 point 9: space/tab/LF/FF/CR are separators; vertical tab
        // (0x0B) is NOT, unlike Rust's `is_ascii_whitespace`.
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
        // §D-1: position counted only among siblings matching `S`.
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

        // §A-4: multi-compound `:has()` argument, now supported via the
        // bounded forward search (`has_forward`) - the (A) exploration and
        // the first cut of this port both left this unimplemented.
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
              </body></html>",
        );
        assert_eq!(select_all(&doc, ":any-link").len(), 1);
        assert_eq!(select_all(&doc, ":link").len(), 1);
        assert_eq!(select_all(&doc, "input:checked").len(), 1);
        assert_eq!(select_all(&doc, "input:required").len(), 1);
        // 6 <input>s total, all but the `required` one are :optional.
        assert_eq!(select_all(&doc, "input:optional").len(), 5);
        assert_eq!(select_all(&doc, "input:read-only").len(), 1);
        // 6 <input>s total, all but the `readonly` one are :read-write.
        assert_eq!(select_all(&doc, "input:read-write").len(), 5);
        assert_eq!(select_all(&doc, "button:disabled").len(), 3); // own attr + 2 inherited
        assert_eq!(select_all(&doc, "button:enabled").len(), 1); // the one under <legend>
        assert_eq!(select_all(&doc, "p:empty").len(), 1);
        assert_eq!(select_all(&doc, "p:blank").len(), 2); // :blank tolerates whitespace-only text
                                                          // :active/:focus/:hover are literal attribute-presence checks (§C-1),
                                                          // not "always false" - none of this fixture's markup has them.
        assert_eq!(select_all(&doc, ":hover").len(), 0);
    }

    #[test]
    fn deeply_nested_is_does_not_grow_the_native_stack() {
        // The one property this module exists to prove (contrast the (A)
        // exploration's sibling test, which crashes in a release build at a
        // nesting depth of only ~2,000-2,500 -
        // notes/css_selectors_crate_migration_plan.ja.md §1.1/§4 Phase 1).
        // 500,000 mirrors the depth already measured safe for Lexbor's own C
        // matcher (§4 Phase 0).
        let doc = parsed(b"<html><body><a>x</a></body></html>");
        let depth = 500_000;
        let nested = format!("{}a{}", ":is(".repeat(depth), ")".repeat(depth));
        assert_eq!(texts(&doc, &nested), ["x"]);
    }

    /// `has_forward` is the one place in this file that DOES use native Rust
    /// recursion (module doc), bounded by `MAX_COMPOUNDS` (64) rather than by
    /// stack-safety - the plan's remaining open item is confirming that bound
    /// is actually safe on Ruby's smallest documented `Fiber` machine stack
    /// (`RUBY_FIBER_MACHINE_STACK_SIZE`, as small as 128 KiB - see
    /// `crate::stack`'s module doc), not just "negligible" by inspection.
    ///
    /// Measured directly: a thread sized to that 128 KiB budget (smaller than
    /// what any real Fiber call would have left after Ruby's, magnus's and
    /// `bridge::gvl`'s own frames - this is a lower bound on the margin, not
    /// the exact in-Ruby number, since the port is not wired into `Node#css`
    /// yet to measure that directly) runs a `:has()` argument built to force
    /// EXACTLY 64 levels of `has_forward` - a distinct class per nesting
    /// level, so a short-circuit on an early mismatch cannot cut the
    /// recursion short. A real stack overflow kills the thread outright
    /// (`join()` returns `Err`, not a wrong `bool`), so this either times out
    /// on the join, or answers `Ok(true)`.
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

    /// Phase 2's differential check: the port and the OLD Lexbor-callback
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
            let new_first = port_select_first(list.node(), parsed_sel.groups());
            let new_matches = matches_any(parsed_sel.groups(), list);

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
}
