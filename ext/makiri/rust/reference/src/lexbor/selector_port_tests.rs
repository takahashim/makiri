//! The selector parser port (`crate::selectors`) held to Lexbor's: the CSS
//! syntax tokenizer token for token, then the parser's AST.

#![allow(unsafe_code)]
#![allow(clippy::disallowed_methods)] /* test fixtures */

use crate::lexbor::abi::consts::STATUS_OK;
use crate::lexbor::abi::{
    lxb_css_syntax_token, lxb_css_syntax_token_consume, lxb_css_syntax_tokenizer_create,
    lxb_css_syntax_tokenizer_destroy, lxb_css_syntax_tokenizer_init,
};
use crate::selectors::token::TokenType;
use crate::selectors::tokenizer::Tokenizer;

/// One token, as both tokenizers are compared on it.
#[derive(Debug, PartialEq)]
struct Dumped {
    ty: u32,
    offset: usize,
    length: usize,
    string: Vec<u8>,
    num: u64,
    is_float: bool,
    have_sign: bool,
    delim: u32,
}

/// Types whose union member is a string (`IDENT` to `WHITESPACE`).
fn is_string(ty: u32) -> bool {
    (TokenType::Ident as u32..=TokenType::Whitespace as u32).contains(&ty)
}

fn is_numeric(ty: u32) -> bool {
    ty == TokenType::Number as u32
        || ty == TokenType::Percentage as u32
        || ty == TokenType::Dimension as u32
}

/// Lexbor's tokens for `input`, up to and including its end of file.
fn lexbor_tokens(input: &[u8]) -> Vec<Dumped> {
    let mut out = Vec::new();
    // SAFETY: create/init/destroy as Lexbor documents them; the three input
    // stores are its inline `buffer_set`; every token is read before the
    // consume that frees it, and only through the member its type names.
    unsafe {
        let tkz = lxb_css_syntax_tokenizer_create();
        assert!(!tkz.is_null());
        assert_eq!(lxb_css_syntax_tokenizer_init(tkz), STATUS_OK);
        let base = input.as_ptr();
        (*tkz).in_begin = base;
        (*tkz).in_p = base;
        (*tkz).in_end = base.wrapping_add(input.len());
        loop {
            let t = lxb_css_syntax_token(tkz);
            assert!(!t.is_null(), "Lexbor's tokenizer failed on {input:?}");
            let t = &*t;
            let ty = t.type_;
            let mut d = Dumped {
                ty,
                offset: t.offset,
                length: t.types.base.length,
                string: Vec::new(),
                num: 0,
                is_float: false,
                have_sign: false,
                delim: 0,
            };
            let s = if is_string(ty) {
                Some(t.types.string)
            } else if ty == TokenType::Dimension as u32 {
                Some(t.types.dimension.str_)
            } else {
                None
            };
            if let Some(s) = s {
                if !s.data.is_null() {
                    d.string = core::slice::from_raw_parts(s.data, s.length).to_vec();
                }
            }
            if is_numeric(ty) {
                let n = t.types.number;
                d.num = n.num.to_bits();
                d.is_float = n.is_float;
                d.have_sign = n.have_sign;
            }
            if ty == TokenType::Delim as u32 {
                d.delim = t.types.delim.character;
            }
            out.push(d);
            lxb_css_syntax_token_consume(tkz);
            if ty == TokenType::Eof as u32 {
                break;
            }
        }
        lxb_css_syntax_tokenizer_destroy(tkz);
    }
    out
}

/// The port's tokens for `input`, up to and including its end of file.
fn port_tokens(input: &[u8]) -> Vec<Dumped> {
    let mut tkz = Tokenizer::new(input);
    let mut out = Vec::new();
    loop {
        let t = tkz
            .token()
            .unwrap_or_else(|| panic!("the port failed on {input:?}"));
        let ty = t.ty as u32;
        let mut d = Dumped {
            ty,
            offset: t.offset,
            length: t.length,
            string: Vec::new(),
            num: 0,
            is_float: false,
            have_sign: false,
            delim: 0,
        };
        if is_string(ty) || ty == TokenType::Dimension as u32 {
            d.string = tkz.string(t.string).to_vec();
        }
        if is_numeric(ty) {
            d.num = t.num.to_bits();
            d.is_float = t.is_float;
            d.have_sign = t.have_sign;
        }
        if ty == TokenType::Delim as u32 {
            d.delim = t.delim;
        }
        out.push(d);
        tkz.consume();
        if t.ty == TokenType::Eof {
            break;
        }
    }
    out
}

/// The port's tokens are compared as Lexbor read them (`compat::as_lexbor`):
/// the tokenizer's own departures (C4 and on) are the stylesheet and selector
/// checks' to admit.
fn same_tokens(input: &[u8]) {
    let lexbor = lexbor_tokens(input);
    let port = crate::compat::as_lexbor(|| port_tokens(input));
    if lexbor != port {
        let at = lexbor
            .iter()
            .zip(&port)
            .position(|(a, b)| a != b)
            .unwrap_or(lexbor.len().min(port.len()));
        panic!(
            "tokens differ on {:?} at token {at}:\n lexbor {:?}\n port   {:?}",
            String::from_utf8_lossy(input),
            lexbor.get(at),
            port.get(at)
        );
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

/// Pieces random inputs are made of: the characters every state turns on.
const PIECES: &[&[u8]] = &[
    b"a",
    b"Z",
    b"_",
    b"-",
    b"--",
    b"0",
    b"7",
    b"12",
    b".",
    b".5",
    b"+",
    b"e",
    b"E",
    b"e+",
    b"e-",
    b"%",
    b"#",
    b"@",
    b"\\",
    b"\\41",
    b"\\0",
    b"\\110000",
    b"\\D800",
    b"\\fffd ",
    b"\\\n",
    b"\\\r\n",
    b"\\'",
    b"'",
    b"\"",
    b"(",
    b")",
    b"[",
    b"]",
    b"{",
    b"}",
    b",",
    b":",
    b";",
    b"<",
    b"<!--",
    b"-->",
    b"!",
    b"|",
    b"*",
    b"~",
    b">",
    b"=",
    b"$",
    b"^",
    b"/",
    b"/*",
    b"*/",
    b" ",
    b"\t",
    b"\n",
    b"\r",
    b"\r\n",
    b"\x0c",
    b"\x0b",
    b"\x00",
    b"\x01",
    b"\x7f",
    b"url(",
    b"URL(",
    b"u",
    b"U+1?",
    b"url( x)",
    b"url('a')",
    "é".as_bytes(),
    "€".as_bytes(),
    "\u{10348}".as_bytes(),
    "\u{2070}".as_bytes(),
    "\u{00d7}".as_bytes(),
    b"\xc3",
    b"\xed\xa0\x80",
    b"\xff",
    b"\x80",
    b"\xe2\x82",
    b"n",
    b"-n",
    b"odd",
    b"even",
    b"99999999999999999999",
    b"1e308",
    b"1e400",
    b"1.0e-400",
    b"123456789012345678901234567890",
];

fn random_input(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.below(16) {
        out.extend_from_slice(rng.pick(PIECES));
    }
    out
}

#[test]
fn the_tokenizer_reads_lexbors_tokens() {
    for input in [
        &b""[..],
        b"div",
        b"div > p.a#b[c='d' i]:nth-child(2n+1 of .x)",
        b"a /* comment */ b",
        b"/* unterminated",
        b"'unterminated",
        b"'bad\nstring'",
        b"12px 1.5em -3 +.5 1e3 1e 1e- 1e+x 2E-2% .5e5",
        b"#-a #1 #\\31 # \\",
        b"url(  x.png  ) url(x y) url('q') url(\\)",
        b"--x -x - -1 -.5 ->",
        b"\\41 bc \\0 \\110000 \\D800",
        "é€\u{10348}".as_bytes(),
        b"\xc3(\xff\xed\xa0\x80",
    ] {
        same_tokens(input);
    }
}

#[test]
fn the_tokenizer_reads_lexbors_tokens_on_random_inputs() {
    let count = std::env::var("SELECTOR_PORT_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let mut rng = Rng(0x05EE_DC55);
    for _ in 0..count {
        same_tokens(&random_input(&mut rng));
    }
}

/* ---- the parser ---- */

use crate::lexbor::selector_diff::{check as same_parse, port_dump};

const SELECTORS: &[&str] = &[
    "*",
    "div",
    "DIV",
    "a b",
    "a  >  b",
    "a+b",
    "a ~ b",
    "a || b",
    "a|b",
    "*|*",
    "|a",
    "ns|*",
    "|*",
    "*|a",
    ".a.b#c",
    "#1",
    "#-a",
    ".1",
    "a, b",
    "a,",
    ",a",
    "a,,b",
    "",
    " ",
    "a ",
    " a",
    ">a",
    "a >",
    "a > > b",
    "[a]",
    "[ a ]",
    "[a=b]",
    "[a='b']",
    "[a=\"b\" i]",
    "[a=b s]",
    "[a=b x]",
    "[a=b ii]",
    "[a|=b]",
    "[a~=b]",
    "[a^=b]",
    "[a$=b]",
    "[a*=b]",
    "[|a]",
    "[*|a]",
    "[ns|a=b]",
    "[a|b|=c]",
    "[a",
    "[a=",
    "[a=b",
    "[a=b i",
    "[=b]",
    "[a==b]",
    "[a=1]",
    "[a= b ]",
    "a[b][c]",
    "[a=b]x",
    ":first-child",
    ":FIRST-CHILD",
    ":root",
    ":hover",
    ":visited",
    ":target",
    ":scope",
    ":foo",
    "::before",
    "::foo",
    "a::before",
    "::x()",
    ":nth-child(2)",
    ":nth-child(2n+1)",
    ":nth-child(-n+3)",
    ":nth-child(odd)",
    ":nth-child(EVEN)",
    ":nth-child(n)",
    ":nth-child(-n)",
    ":nth-child(+n)",
    ":nth-child(+ n)",
    ":nth-child(n-1)",
    ":nth-child(n - 1)",
    ":nth-child(n -1)",
    ":nth-child(n- 1)",
    ":nth-child(2n - 1)",
    ":nth-child(3n+ 2)",
    ":nth-child(1.5)",
    ":nth-child(2.0n)",
    ":nth-child()",
    ":nth-child( 2 )",
    ":nth-child(2 of .a)",
    ":nth-child(2n of .a, .b)",
    ":nth-child(of .a)",
    ":nth-child(2 of)",
    ":nth-last-child(1)",
    ":nth-of-type(2)",
    ":nth-last-of-type(2n)",
    ":nth-of-type(2 of .a)",
    ":nth-col(1)",
    ":nth-child(n-a)",
    ":nth-child(9223372036854774784)",
    ":nth-child(-9223372036854775807)",
    ":nth-child(99999999999999999999)",
    ":nth-child(n-99999999999999999999)",
    ":nth-child(1e3)",
    ":nth-child(10n-3)",
    ":nth-child(n+-3)",
    ":not(a)",
    ":not(a, b)",
    ":not()",
    ":not(:not(a))",
    ":not(a b > c)",
    ":not(a,)",
    ":not(,a)",
    ":is(a, b)",
    ":is()",
    ":is(a, $, b)",
    ":is($)",
    ":where(a, .b)",
    ":where(#x)",
    ":where($)",
    ":has(a)",
    ":has(> a)",
    ":has(+ a, ~ b)",
    ":has(a b)",
    ":has()",
    ":has(:has(a))",
    ":has(> > a)",
    ":current(a)",
    ":dir(ltr)",
    ":lang(en)",
    ":lexbor-contains(a)",
    ":contains('x')",
    ":is(a",
    ":not(a",
    "a:is(.b, .c) > d:not(.e)",
    ":is(:is(:is(a)))",
    ":where(a, :is(b, c))",
    "a:has(b, c) d",
    ":nth-child(2 of :is(a, b))",
    ":is(a,, b)",
    ":is(a, (b), c)",
    ":is([a, b)",
    "a /* c */ b",
    "a/**/b",
    "\\61",
    "\\31 23",
    "#\\31",
    ".\\-",
    "a\\:b",
    "é",
    ".é",
    "#€",
    "a(b)",
    "a)",
    "a]",
    "a}",
    "{a}",
    "a;b",
    "a!b",
    "@a",
    "a@b",
    "url(a)",
    "a url(b)",
    ":is(url(x))",
    "--a",
    "-a",
    "-",
    "_a",
    "a--b",
    "a\tb",
    "a\nb",
    "a\rb",
    "a\u{c}b",
];

/// Where the port answers otherwise on purpose (docs/parser_differences.ja.md,
/// S2): `An+B` values Lexbor converts with C's undefined double-to-long cast.
/// The port saturates; Lexbor's answer is the platform's.
const DIVERGENT: &[(&str, &str)] = &[(
    ":nth-child(9223372036854775807)",
    "L200[Descendant<nth-child>N(false,false,0,9223372036854775807) ]",
)];

#[test]
fn the_parser_builds_lexbors_selectors() {
    for s in SELECTORS {
        same_parse(s);
    }
    for (s, port) in DIVERGENT {
        assert_eq!(port_dump(s).as_deref(), Some(*port), "{s:?}");
    }
}

/// A random selector-ish string: mostly grammatical, with nesting, odd
/// whitespace and stray tokens, so both the parse and the error recovery are
/// exercised.
fn random_selector(rng: &mut Rng, depth: u32) -> String {
    const NAMES: &[&str] = &[
        "a", "div", "LI", "x-y", "\\61", "é", "-z", "_", "svg", "n", "of", "odd",
    ];
    const PSEUDO: &[&str] = &[
        "first-child",
        "last-child",
        "only-child",
        "empty",
        "root",
        "hover",
        "checked",
        "disabled",
        "enabled",
        "link",
        "any-link",
        "visited",
        "target",
        "scope",
        "focus-within",
        "foo",
        "FIRST-OF-TYPE",
        "blank",
    ];
    const FUNCS: &[&str] = &[
        "not",
        "is",
        "where",
        "has",
        "nth-child",
        "nth-last-child",
        "nth-of-type",
        "nth-last-of-type",
        "current",
        "dir",
        "lang",
        "nth-col",
        "lexbor-contains",
        "foo",
        "NOT",
        "Is",
    ];
    const ANB: &[&str] = &[
        "1",
        "2n",
        "2n+1",
        "-n+3",
        "odd",
        "even",
        "n",
        "+n",
        "-n",
        "n-1",
        "n - 1",
        "2n -1",
        "+ 1",
        "1.5",
        "",
        " 3 ",
        "n+",
        "3n-",
        "-2n-2",
        "N",
        "0n+0",
        "99999999999999999999",
        "+5",
        "-5",
        "n- 2",
        "+n+1",
    ];
    const WS: &[&str] = &["", "", "", " ", "  ", "\t", "\n", "/**/", " /* c */ "];
    const COMB: &[&str] = &[" ", ">", "+", "~", "||", " > ", " + ", " ~ ", "|", ">>", ""];
    const STRAY: &[&str] = &[
        ",", ")", "(", "[", "]", "$", "!", "{", "}", ";", "@x", "'", "\"", "#", ".", ":", "::", "1",
    ];

    let mut out = String::new();
    let groups = 1 + rng.below(3);
    for g in 0..groups {
        if g > 0 {
            out.push_str(rng.pick(WS));
            out.push(',');
            out.push_str(rng.pick(WS));
        }
        let compounds = 1 + rng.below(3);
        for c in 0..compounds {
            if c > 0 {
                out.push_str(rng.pick(WS));
                out.push_str(rng.pick(COMB));
                out.push_str(rng.pick(WS));
            }
            let simples = 1 + rng.below(3);
            for i in 0..simples {
                let kind = rng.below(if depth < 3 { 12 } else { 9 });
                match kind {
                    0 if i == 0 => out.push_str(rng.pick(NAMES)),
                    1 if i == 0 => out.push('*'),
                    2 if i == 0 => {
                        out.push_str(rng.pick(&["*", "", "ns"]));
                        out.push('|');
                        out.push_str(rng.pick(&["*", "a", "B"]));
                    }
                    3 => {
                        out.push('#');
                        out.push_str(rng.pick(NAMES));
                    }
                    4 => {
                        out.push('.');
                        out.push_str(rng.pick(NAMES));
                    }
                    5 => {
                        out.push('[');
                        out.push_str(rng.pick(WS));
                        out.push_str(rng.pick(&["", "", "|", "*|", "ns|"]));
                        out.push_str(rng.pick(NAMES));
                        out.push_str(rng.pick(WS));
                        if rng.below(3) > 0 {
                            out.push_str(
                                rng.pick(&["=", "~=", "|=", "^=", "$=", "*=", "==", "!="]),
                            );
                            out.push_str(rng.pick(WS));
                            out.push_str(rng.pick(&["b", "'b c'", "\"\"", "1", "x-y", "'", "é"]));
                            out.push_str(rng.pick(WS));
                            out.push_str(rng.pick(&["", "", "i", "S", "x", "ii"]));
                            out.push_str(rng.pick(WS));
                        }
                        if rng.below(10) > 0 {
                            out.push(']');
                        }
                    }
                    6 | 7 => {
                        out.push(':');
                        out.push_str(rng.pick(PSEUDO));
                    }
                    8 => {
                        out.push_str("::");
                        out.push_str(rng.pick(&["before", "after", "x", "selection"]));
                    }
                    _ => {
                        let f = *rng.pick(FUNCS);
                        out.push(':');
                        out.push_str(f);
                        out.push('(');
                        out.push_str(rng.pick(WS));
                        if f.to_ascii_lowercase().starts_with("nth") {
                            out.push_str(rng.pick(ANB));
                            if rng.below(3) == 0 {
                                out.push_str(rng.pick(&[" of ", " OF ", " of", "of "]));
                                out.push_str(&random_selector(rng, depth + 1));
                            }
                        } else if f.eq_ignore_ascii_case("has") && rng.below(2) == 0 {
                            out.push_str(rng.pick(COMB));
                            out.push_str(&random_selector(rng, depth + 1));
                        } else if rng.below(8) > 0 {
                            out.push_str(&random_selector(rng, depth + 1));
                        }
                        out.push_str(rng.pick(WS));
                        if rng.below(10) > 0 {
                            out.push(')');
                        }
                    }
                }
                if rng.below(30) == 0 {
                    out.push_str(rng.pick(STRAY));
                }
            }
        }
    }
    out
}

/* 2,000 inputs by default (debug, about half a minute): the selector_diff
 * and stylesheet_diff fuzz targets search the same comparison far deeper,
 * nightly under ASan. SELECTOR_PORT_COUNT runs more here. */
#[test]
fn the_parser_builds_lexbors_selectors_on_random_inputs() {
    let count = std::env::var("SELECTOR_PORT_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000);
    let mut rng = Rng(0xC55_5E1);
    for _ in 0..count {
        let s = random_selector(&mut rng, 0);
        same_parse(&s);
    }
}

/// Every selector the specs and tests use: the double-quoted arguments of
/// `css(`, `at_css(`, `matches?(`, `search(` and `at(` in `spec/`, and of the
/// Rust tests' selector helpers.
#[test]
fn the_parser_builds_lexbors_selectors_on_the_repository_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let mut files = Vec::new();
    let mut stack = vec![
        root.join("spec"),
        root.join("ext/makiri/rust/src"),
        root.join("ext/makiri/rust/engine/src"),
        root.join("ext/makiri/rust/reference/src"),
    ];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("data") {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rb" || x == "rs") {
                files.push(p);
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for call in [
            "css(\"",
            "at_css(\"",
            "matches?(\"",
            "search(\"",
            "selector(\"",
            "select_all(&doc, \"",
            "parse_sel(\"",
        ] {
            let mut rest = text.as_str();
            while let Some(i) = rest.find(call) {
                rest = &rest[i + call.len()..];
                let Some(end) = rest.find('"') else {
                    break;
                };
                let lit = &rest[..end];
                if !lit.contains("#{") {
                    seen.insert(lit.to_string());
                }
            }
        }
    }
    assert!(seen.len() > 200, "found only {} selectors", seen.len());
    for s in &seen {
        same_parse(s);
    }
    eprintln!("selector corpus: {} selectors", seen.len());
}

/// Parse time against Lexbor's, on typical selectors (a measurement, not a
/// check): `cargo test --release ... selector_parse_speed -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn selector_parse_speed() {
    use crate::gvl::Gvl;
    use crate::lexbor::css_parser;
    use crate::text::VerifiedText;
    use std::time::Instant;
    const SAMPLE: &[&str] = &[
        "div",
        "#main",
        ".item",
        "ul li.item",
        "main > section a[href^='/p']",
        "li:nth-child(2n+1 of .x)",
        ":is(li, p) > a.link:not(.ext)",
        "li:has(> span.meta), p ~ p:last-of-type",
    ];
    let rounds = 50_000;
    for s in SAMPLE {
        let t = Instant::now();
        for _ in 0..rounds {
            let p = crate::selectors::parse(s.as_bytes()).expect("parses");
            core::hint::black_box(&p);
        }
        let port = t.elapsed();
        let gvl = Gvl::exclusive();
        let t = Instant::now();
        for _ in 0..rounds {
            let text = VerifiedText::from_bytes(s.as_bytes()).expect("text");
            let p = css_parser::parse(&gvl, text).unwrap_or_else(|_| panic!("parses"));
            core::hint::black_box(&p);
        }
        let lexbor = t.elapsed();
        eprintln!(
            "{s:48} port {:6.0} ns  lexbor {:6.0} ns  ratio {:.2}",
            port.as_nanos() as f64 / rounds as f64,
            lexbor.as_nanos() as f64 / rounds as f64,
            port.as_secs_f64() / lexbor.as_secs_f64()
        );
    }
}

/* ---- the stylesheet parser ---- */

fn same_sheet(css: &str) {
    crate::lexbor::stylesheet_diff::check(css.as_bytes());
}

#[test]
fn the_stylesheet_parser_builds_lexbors_rules() {
    for css in [
        "",
        " ",
        "a{}",
        "a { color: red }",
        "a{color:red;}",
        "a { color: red; background: blue }",
        "a, b > c { margin: 1px   2px ; padding: 0 }",
        "a { color: RED ! important; x: y !important }",
        "a { x: y !important; z: w!IMPORTANT }",
        "a { x: y ! important }",
        "a { x: y !important x; }",
        "a { x: y !importantx }",
        "a { x: !important }",
        "a { x: y !important",
        "a { x: y",
        "a { x: y ",
        "a { x",
        "a { x: (a; b) ; c: d }",
        "a { x: [a; b] ; c: d }",
        "a { x: {a; b} ; c: d }",
        "a { x: f(a; b) ; c: d }",
        "a { x: f(a}; c: d }",
        "a { x: ); c: d }",
        "a { b {} c:d }",
        "a { c: d; b {} e: f }",
        "a { c: d; b; e: f }",
        "a { @media x { b {} } c: d }",
        "a { c: d; @foo; e: f }",
        "a { 1: 2; c: d }",
        "a { ;;; c: d;;; }",
        "a { c : d }",
        "a { c/**/: d }",
        "a { --X: a   b ; --y:; --z: {a} }",
        "p{Col\\6fr:x}",
        "a:dir(ltr){} b{}",
        "a:foo(x){c:d}",
        "a::before { c: d }",
        "a:hover, b::after { c: d }",
        "{ c: d }",
        ", { c: d }",
        "a,, b { }",
        "a } b { c: d }",
        "} a { c: d }",
        "a { } } b { }",
        "a; b { c: d }",
        "<!-- a { b: c } -->",
        "a <!-- { b: c }",
        "@media screen { a { b: c } }",
        "@MEDIA screen;",
        "@media;",
        "@media { a {} }",
        "@media screen and (min-width: 600px) { a { b: c } d { e: f } }",
        "@media x { @media y { a { b: c } } }",
        "@media x { a { b: c } ",
        "@media x",
        "@font-face { src: url(x) }",
        "@font-face x { src: y }",
        "@font-face;",
        "@Font-Face { a: b }",
        "@namespace svg url(http://www.w3.org/2000/svg);",
        "@namespace x {a{}}",
        "@import url(x.css) screen;",
        "@charset \"utf-8\";",
        "@foo bar { a { b: c } }",
        "@Foo; x{y:z}",
        "@foo bar baz; @qux { }",
        "@supports (display: grid) { a { display: grid } }",
        "@keyframes k { from { a: b } 50% { a: c } to { a: d } }",
        "@layer a, b;",
        "@page :first { margin: 1in }",
        "@foo { a: b; c { d: e } }",
        "@foo [ { ] } a { b: c }",
        "@foo ( { ) } a { b: c }",
        "@ { }",
        "@\\66oo { }",
        "a { b: c } @foo",
        "a { b: c } @foo {",
        "a[x=\"}\"] { b: \"}\" }",
        "a { b: url(}) }",
        "a { b: url( x ) }",
        "a { b: 'unterminated",
        "a { b: \"x\ny\" }",
        "a /* c */ { b /* c */ : /* c */ d /* c */ ; }",
        "a { b: c /* !important */ }",
        "a { b: c /**/!important }",
        "a { b: c! important }",
        "a { b: c !important; }",
        "a { b: c !important}",
        "a { b: c\t!important\n}",
        "a { width: calc(1px + 2px) !important }",
        "a { color: rgb(1, 2, 3) }",
        "a { font-family: \"A B\", serif }",
        "a { b: c; ; d: e }",
        "a:is(b, c) > d:not(.e) { f: g }",
        "a:has(> b) { c: d }",
        "a:nth-child(2n+1 of .x) { c: d }",
        "a:lexbor-contains(x) { b: c }",
        "a { b: c:lexbor-contains(1 2) }",
        "\u{e9} { \u{e9}: \u{e9} }",
        "a{b:c}a{b:c}a{b:c}",
        "a{b:c}\n\n@media print{a{b:c}}\n",
    ] {
        same_sheet(css);
    }
    // Nested past the dumps' depth (a fuzz find in the dumpers, not the
    // parsers): both stop at the same depth.
    same_sheet(&format!("{}-\\+\n{}+\n", "{".repeat(24), "{".repeat(9)));
    same_sheet(&"a{".repeat(300));
    same_sheet(&"@media x{".repeat(300));
}

fn random_sheet(rng: &mut Rng, depth: u32, out: &mut String) {
    const AT: &[&str] = &[
        "@media",
        "@MEDIA",
        "@font-face",
        "@namespace",
        "@foo",
        "@supports",
        "@import",
        "@keyframes",
        "@",
    ];
    const PRELUDE: &[&str] = &[
        "",
        " ",
        " screen",
        " (min-width: 1px)",
        " x y",
        " url(x)",
        " [",
        " (",
        " {",
        " }",
        " ;",
        " \"s\"",
    ];
    const NAMES: &[&str] = &[
        "color",
        "COLOR",
        "margin",
        "--x",
        "--X",
        "-webkit-a",
        "col\\6fr",
        "font-family",
        "display",
        "width",
        "x",
        "1",
        "\u{e9}",
        "a b",
    ];
    const VALUES: &[&str] = &[
        "red",
        "1px",
        "  2px",
        "\"s\"",
        "'s'",
        "url(x)",
        "url( \"a\" )",
        "calc(1px + 2px)",
        "!important",
        "! important",
        "!IMPORTANT",
        "/*c*/",
        "\n",
        "\t",
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
        ";",
        "rgb(1, 2, 3)",
        "\\",
        "#fff",
        "50%",
        "U+00-ff",
        "a:b",
        "!",
        ",",
        " ",
        "<!--",
        "-->",
        "@x",
    ];
    let n = 1 + rng.below(4);
    for _ in 0..n {
        match rng.below(8) {
            0..=2 => {
                out.push_str(&random_selector(rng, 0));
                out.push_str(rng.pick(&["{", " { ", "{\n"]));
                random_block(rng, depth, out, NAMES, VALUES);
                if rng.below(8) != 0 {
                    out.push('}');
                }
            }
            3 | 4 => {
                out.push_str(rng.pick(AT));
                out.push_str(rng.pick(PRELUDE));
                if rng.below(3) == 0 {
                    out.push(';');
                } else {
                    out.push('{');
                    if depth < 3 && rng.below(2) == 0 {
                        random_sheet(rng, depth + 1, out);
                    } else {
                        random_block(rng, depth, out, NAMES, VALUES);
                    }
                    if rng.below(8) != 0 {
                        out.push('}');
                    }
                }
            }
            5 => out.push_str(rng.pick(&[" ", "\n", ";", "}", "<!--", "-->", "/*x*/"])),
            _ => {
                for _ in 0..rng.below(4) {
                    out.push_str(rng.pick(VALUES));
                }
            }
        }
    }
}

fn random_block(rng: &mut Rng, depth: u32, out: &mut String, names: &[&str], values: &[&str]) {
    for _ in 0..rng.below(5) {
        match rng.below(10) {
            0 if depth < 3 => {
                random_sheet(rng, depth + 1, out);
            }
            1 => out.push_str(rng.pick(&[";", " ", "@foo;", "x", "}"])),
            _ => {
                out.push_str(rng.pick(names));
                out.push_str(rng.pick(&[":", " : ", ":", " :", "", "/**/:"]));
                for _ in 0..rng.below(4) {
                    out.push_str(rng.pick(values));
                    if rng.below(2) == 0 {
                        out.push(' ');
                    }
                }
                out.push_str(rng.pick(&[";", ";", " ; ", "", "\n"]));
            }
        }
    }
}

/* 2,000 inputs by default (debug, about half a minute): the selector_diff
 * and stylesheet_diff fuzz targets search the same comparison far deeper,
 * nightly under ASan. SELECTOR_PORT_COUNT runs more here. */
#[test]
fn the_stylesheet_parser_builds_lexbors_rules_on_random_inputs() {
    let count = std::env::var("SELECTOR_PORT_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000);
    let mut rng = Rng(0x5EE_75EE7);
    for _ in 0..count {
        let mut css = String::new();
        random_sheet(&mut rng, 0, &mut css);
        same_sheet(&css);
    }
}

/// Token soup: the tokenizer's pieces in any order, so that recovery is
/// tried from every place.
#[test]
fn the_stylesheet_parser_builds_lexbors_rules_on_token_soup() {
    let count = std::env::var("SELECTOR_PORT_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let mut rng = Rng(0x50_0B);
    const SOUP: &[&str] = &[
        "a",
        "b",
        " ",
        "{",
        "}",
        ";",
        ":",
        "!",
        "important",
        "@media",
        "@x",
        "(",
        ")",
        "[",
        "]",
        ",",
        ".c",
        "#i",
        "url(",
        "\"",
        "'",
        "/*",
        "*/",
        "\\",
        "<!--",
        "-->",
        "--v",
        "1",
        "f(",
        "::",
        ">",
        "\n",
        "@font-face",
        "@namespace",
    ];
    for _ in 0..count {
        let mut css = String::new();
        for _ in 0..rng.below(24) {
            css.push_str(rng.pick(SOUP));
        }
        same_sheet(&css);
    }
}
