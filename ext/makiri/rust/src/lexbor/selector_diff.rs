//! The ported selector parser (`crate::selectors`) against Lexbor's
//! (`lexbor::css_parser`, behind `contains_guard` as Makiri runs it), as data:
//! both parse the same selector and their views - every list's specificity,
//! every simple selector's combinator, name, namespace and kind, and what the
//! kind carries, nested lists included - are dumped and compared
//! (`docs/rewrite_parser.ja.md`, stage 5). `lexbor::selector_port_tests` and
//! the `selector_diff` fuzz target use it; it is built for them alone
//! (`lexbor-reference`).

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// The same dump over either parser's views: the two modules expose the same
/// API, so one body serves both.
macro_rules! dumper {
    ($name:ident, $($m:ident)::+) => {
        pub(crate) fn $name(lists: $($m)::+::Lists<'_>, out: &mut String) {
            use $($m)::+ as v;
            for list in lists {
                out.push_str(&format!("L{:x}[", list.specificity()));
                let mut cur = list.first();
                while let Some(s) = cur {
                    out.push_str(&format!("{:?}", s.combinator()));
                    out.push_str(&format!("<{}", String::from_utf8_lossy(s.name())));
                    if let Some(ns) = s.ns() {
                        out.push_str(&format!("|ns={}", String::from_utf8_lossy(ns)));
                    }
                    out.push('>');
                    match s.simple() {
                        v::Simple::Universal => out.push('U'),
                        v::Simple::Type => out.push('T'),
                        v::Simple::Id => out.push('I'),
                        v::Simple::Class => out.push('C'),
                        v::Simple::Attribute(a) => out.push_str(&format!(
                            "A({:?},{:?},{:?})",
                            a.op,
                            a.case,
                            a.value.map(|x| String::from_utf8_lossy(x).into_owned())
                        )),
                        v::Simple::PseudoClass(p) => out.push_str(&format!("P({p:?})")),
                        v::Simple::PseudoClassFunction(f) => match f {
                            v::FunctionArg::Nth { from_end, of_type, anb } => {
                                out.push_str(&format!("N({from_end},{of_type}"));
                                if let Some(n) = anb {
                                    out.push_str(&format!(",{},{}", n.a, n.b));
                                    if let Some(of) = n.of {
                                        out.push_str(",of:");
                                        $name(of, out);
                                    }
                                }
                                out.push(')');
                            }
                            v::FunctionArg::Selectors { pseudo, lists } => {
                                out.push_str(&format!("S({pseudo:?}:"));
                                $name(lists, out);
                                out.push(')');
                            }
                            v::FunctionArg::Contains => out.push_str("Contains"),
                            v::FunctionArg::Current(lists) => {
                                out.push_str("Cur(");
                                $name(lists, out);
                                out.push(')');
                            }
                            v::FunctionArg::Other => out.push_str("F?"),
                        },
                        v::Simple::PseudoElement => {
                            out.push_str(&format!("E({})", s.is_pseudo_element_function()))
                        }
                        v::Simple::Other => out.push('?'),
                    }
                    out.push(' ');
                    cur = s.next();
                }
                out.push(']');
            }
        }
    };
}

dumper!(dump_lexbor, crate::lexbor::css_parser);

dumper!(dump_port, crate::selectors::ast);

/// What a parser made of a selector: its dump, or that it refused it.
pub fn lexbor_dump(selector: &str) -> Option<String> {
    use crate::gvl::Gvl;
    use crate::lexbor::css_parser;
    use crate::text::VerifiedText;
    let gvl = Gvl::exclusive();
    let text = VerifiedText::from_bytes(selector.as_bytes())?;
    let out = match css_parser::parse(&gvl, text) {
        Ok(parsed) => {
            let mut out = String::new();
            dump_lexbor(parsed.groups(), &mut out);
            Some(out)
        }
        Err(css_parser::ParseError::Syntax) => None,
        Err(_) => panic!("Lexbor's parser failed on {selector:?}"),
    };
    out
}

pub fn port_dump(selector: &str) -> Option<String> {
    use crate::selectors::Status;
    match crate::selectors::parse(selector.as_bytes()) {
        Ok(parsed) => {
            let mut out = String::new();
            dump_port(parsed.groups(), &mut out);
            Some(out)
        }
        Err(Status::ErrorMemoryAllocation | Status::ErrorOverflow | Status::ErrorWrongArgs) => {
            panic!("the port failed on {selector:?}")
        }
        Err(_) => None,
    }
}

/// The `An+B` values that come only from saturating a number: Lexbor
/// converts a double of 2^63 with C's undefined cast and the port saturates
/// it (docs/parser_differences.ja.md, S2), so where either answers one of
/// these the two are not compared on it.
const EXTREMES: [&str; 3] = [
    "-9223372036854775808",
    "-9223372036854775807",
    "9223372036854775807",
];

fn without_extremes(s: Option<String>) -> Option<String> {
    s.map(|mut s| {
        for x in EXTREMES {
            s = s.replace(x, "EXTREME");
        }
        s
    })
}

/// Panic unless both parsers make the same of `selector`, which must be
/// what Makiri parses: UTF-8 with no NUL.
pub fn check(selector: &str) {
    let lexbor = without_extremes(lexbor_dump(selector));
    let port = without_extremes(port_dump(selector));
    assert_eq!(lexbor, port, "the parsers differ on {selector:?}");
}
