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
        ] {
            rewritten(s);
        }
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
    ];

    #[test]
    fn nothing_the_guard_keeps_makes_lexbor_fail() {
        let gvl = crate::gvl::Gvl::exclusive();
        let parts = ParserParts::build().expect("lexbor css parser");
        let parser = parts.into_parser();
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
                    list.is_some(),
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
