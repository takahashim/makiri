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
