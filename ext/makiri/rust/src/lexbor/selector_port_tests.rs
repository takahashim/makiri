//! The selector parser port (`crate::selectors`) held to Lexbor's: the CSS
//! syntax tokenizer token for token, then the parser's AST.

#![allow(unsafe_code)]

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

fn same_tokens(input: &[u8]) {
    let lexbor = lexbor_tokens(input);
    let port = port_tokens(input);
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
    let mut rng = Rng(0x5EED_C55);
    for _ in 0..count {
        same_tokens(&random_input(&mut rng));
    }
}
