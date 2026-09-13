//! XPath 1.0 lexer (mkr_xpath_lex.c).
//!
//! Context-free by design: the parser resolves the keywords that are names in
//! one position and operators in another (`and`, `or`, `div`, `mod`, `node()`)
//! by lookahead.
//!
//! The C version reads the input only through a bounded reader (`mkr_span_t`), a
//! discipline the build lints for, because a raw cursor in a byte scanner is
//! where out-of-bounds reads come from. Here the input is a slice and the cursor
//! an index, so every read is checked by the language and the discipline needs
//! no enforcing.

#![forbid(unsafe_code)]

use super::number;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Eof,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDot,
    At,
    Comma,
    Pipe,
    Slash,
    DSlash,
    Plus,
    Minus,
    Star,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    ColonColon,
    Dollar,
    Number,
    Literal,
    /// NCName (no colon)
    Name,
    /// prefix:local, or prefix:*
    QName,
}

/// One token. `off`/`len` index the input slice, so the text stays borrowed from
/// the caller's expression buffer exactly as the C token did.
#[derive(Clone, Copy)]
pub struct Token {
    pub kind: Tok,
    pub off: usize,
    pub len: usize,
    /// Valid for `Tok::Number`.
    pub num: f64,
}

impl Token {
    pub fn text<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        &src[self.off..self.off + self.len]
    }
}

/// What went wrong, for the parser to turn into an `mkr_xpath_error_t`.
pub enum LexErr {
    ExpectedNumber,
    UnterminatedString,
    InvalidUtf8Literal,
    UnexpectedChar(u8),
}

/* ---- NCName code-point classes ---- */

/// XPath 1.0 §3.7 -> Namespaces in XML -> the XML 1.0 (5th ed.) Name
/// production, minus ':'. The definition browsers and libxml2 track.
fn is_ncname_start_cp(c: u32) -> bool {
    c == '_' as u32
        || (c >= 'A' as u32 && c <= 'Z' as u32)
        || (c >= 'a' as u32 && c <= 'z' as u32)
        || (0xC0..=0xD6).contains(&c)
        || (0xD8..=0xF6).contains(&c)
        || (0xF8..=0x2FF).contains(&c)
        || (0x370..=0x37D).contains(&c)
        || (0x37F..=0x1FFF).contains(&c)
        || (0x200C..=0x200D).contains(&c)
        || (0x2070..=0x218F).contains(&c)
        || (0x2C00..=0x2FEF).contains(&c)
        || (0x3001..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFFD).contains(&c)
        || (0x10000..=0xEFFFF).contains(&c)
}

fn is_ncname_cont_cp(c: u32) -> bool {
    is_ncname_start_cp(c)
        || c == '-' as u32
        || c == '.' as u32
        || (c >= '0' as u32 && c <= '9' as u32)
        || c == 0xB7
        || (0x0300..=0x036F).contains(&c)
        || (0x203F..=0x2040).contains(&c)
}

/// Strict one-codepoint decode at `s[0..]`: the length and the value, or None
/// for an empty slice or ill-formed UTF-8 (overlong, surrogate, out of range).
fn decode1(s: &[u8]) -> Option<(usize, u32)> {
    let b0 = *s.first()?;
    let (need, mut cp): (usize, u32) = match b0 {
        0x00..=0x7F => return Some((1, b0 as u32)),
        0xC2..=0xDF => (2, (b0 & 0x1F) as u32),
        0xE0..=0xEF => (3, (b0 & 0x0F) as u32),
        0xF0..=0xF4 => (4, (b0 & 0x07) as u32),
        _ => return None, /* continuation byte, or an overlong/out-of-range lead */
    };
    if s.len() < need {
        return None;
    }
    for &b in &s[1..need] {
        if !(0x80..=0xBF).contains(&b) {
            return None;
        }
        cp = (cp << 6) | (b & 0x3F) as u32;
    }
    let ok = match need {
        2 => cp >= 0x80,
        3 => cp >= 0x800 && !(0xD800..=0xDFFF).contains(&cp),
        _ => (0x10000..=0x10FFFF).contains(&cp),
    };
    if ok {
        Some((need, cp))
    } else {
        None
    }
}

/// Byte length of the NCName character at `s[0..]`, or 0. `start` selects
/// NameStartChar over NameChar.
fn ncname_char(s: &[u8], start: bool) -> usize {
    match decode1(s) {
        Some((n, cp))
            if (if start {
                is_ncname_start_cp(cp)
            } else {
                is_ncname_cont_cp(cp)
            }) =>
        {
            n
        }
        _ => 0,
    }
}

/// XPath 1.0 whitespace: S = #x20 | #x9 | #xD | #xA only - NOT C isspace(),
/// which also accepts \v and \f.
#[inline]
pub fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r' || b == b'\n'
}

/* ---- the lexer ---- */

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    pub tok: Token,
}

impl<'a> Lexer<'a> {
    /// Build a lexer positioned on the first token, or the error that stopped it.
    pub fn new(src: &'a [u8]) -> Result<Self, LexErr> {
        let mut l = Lexer {
            src,
            pos: 0,
            tok: Token {
                kind: Tok::Eof,
                off: 0,
                len: 0,
                num: 0.0,
            },
        };
        l.tok = l.next_token()?;
        Ok(l)
    }

    pub fn src(&self) -> &'a [u8] {
        self.src
    }

    pub fn advance(&mut self) -> Result<(), LexErr> {
        self.tok = self.next_token()?;
        Ok(())
    }

    /// The next non-whitespace byte after the current token, without consuming
    /// anything - the parser's "is this NAME followed by '('?" lookahead.
    pub fn peek_nonws(&self) -> Option<u8> {
        self.src[self.pos..].iter().copied().find(|&b| !is_ws(b))
    }

    /// True when the current token is the NAME `word` (the operator keywords
    /// `and` / `or` / `div` / `mod`, which the lexer cannot tell from a name).
    pub fn tok_is_word(&self, word: &[u8]) -> bool {
        self.tok.kind == Tok::Name && self.tok.text(self.src) == word
    }

    fn rest(&self) -> &'a [u8] {
        &self.src[self.pos..]
    }

    fn at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn punct(&mut self, kind: Tok, len: usize) -> Token {
        let t = Token {
            kind,
            off: self.pos,
            len,
            num: 0.0,
        };
        self.pos += len;
        t
    }

    fn next_token(&mut self) -> Result<Token, LexErr> {
        while self.at(0).is_some_and(is_ws) {
            self.pos += 1;
        }
        let c = match self.at(0) {
            None => {
                return Ok(Token {
                    kind: Tok::Eof,
                    off: self.pos,
                    len: 0,
                    num: 0.0,
                })
            }
            Some(c) => c,
        };
        let c1 = self.at(1);

        let two = match (c, c1) {
            (b'/', Some(b'/')) => Some(Tok::DSlash),
            (b'.', Some(b'.')) => Some(Tok::DotDot),
            (b':', Some(b':')) => Some(Tok::ColonColon),
            (b'!', Some(b'=')) => Some(Tok::Ne),
            (b'<', Some(b'=')) => Some(Tok::Le),
            (b'>', Some(b'=')) => Some(Tok::Ge),
            _ => None,
        };
        if let Some(kind) = two {
            return Ok(self.punct(kind, 2));
        }

        let one = match c {
            b'(' => Some(Tok::LParen),
            b')' => Some(Tok::RParen),
            b'[' => Some(Tok::LBracket),
            b']' => Some(Tok::RBracket),
            b'@' => Some(Tok::At),
            b',' => Some(Tok::Comma),
            b'|' => Some(Tok::Pipe),
            b'/' => Some(Tok::Slash),
            b'+' => Some(Tok::Plus),
            b'-' => Some(Tok::Minus),
            b'*' => Some(Tok::Star),
            b'=' => Some(Tok::Eq),
            b'<' => Some(Tok::Lt),
            b'>' => Some(Tok::Gt),
            b'$' => Some(Tok::Dollar),
            /* '.' is DOT unless a digit follows, which makes it a Number. */
            b'.' if !c1.is_some_and(|b| b.is_ascii_digit()) => Some(Tok::Dot),
            _ => None,
        };
        if let Some(kind) = one {
            return Ok(self.punct(kind, 1));
        }

        if c == b'\'' || c == b'"' {
            return self.lex_string(c);
        }
        if c.is_ascii_digit() || c == b'.' {
            return self.lex_number();
        }
        if ncname_char(self.rest(), true) > 0 {
            return Ok(self.lex_name());
        }
        Err(LexErr::UnexpectedChar(c))
    }

    fn lex_number(&mut self) -> Result<Token, LexErr> {
        /* Grammar-exact: only the bytes matching the Number production are
         * consumed, so "0x1A" lexes as NUMBER 0 then NAME "x1A" and "1e3" as
         * NUMBER 1 then NAME "e3" - libxml2 / browser behaviour. */
        let extent = number::extent(self.rest());
        if extent == 0 {
            return Err(LexErr::ExpectedNumber);
        }
        let num = number::from_extent(&self.rest()[..extent]);
        let t = Token {
            kind: Tok::Number,
            off: self.pos,
            len: extent,
            num,
        };
        self.pos += extent;
        Ok(t)
    }

    fn lex_string(&mut self, quote: u8) -> Result<Token, LexErr> {
        let start = self.pos + 1;
        let len = match self.src[start..].iter().position(|&b| b == quote) {
            Some(n) => n,
            None => return Err(LexErr::UnterminatedString),
        };
        /* Validate here, once, so every character-wise string function
         * (translate, substring, string-length) can assume well-formed input and
         * an invalid literal fails closed as a SyntaxError. */
        if core::str::from_utf8(&self.src[start..start + len]).is_err() {
            return Err(LexErr::InvalidUtf8Literal);
        }
        self.pos = start + len + 1; /* content + closing quote */
        Ok(Token {
            kind: Tok::Literal,
            off: start,
            len,
            num: 0.0,
        })
    }

    fn lex_name(&mut self) -> Token {
        let start = self.pos;
        loop {
            let n = ncname_char(self.rest(), false);
            if n == 0 {
                break;
            }
            self.pos += n;
        }
        /* A QName NameTest is `prefix:local` or `prefix:*` (§2.3); the ':' must
         * not be the '::' axis separator. */
        let colon_starts_qname = self.at(0) == Some(b':')
            && self.at(1) != Some(b':')
            && (self.at(1) == Some(b'*') || ncname_char(&self.src[self.pos + 1..], true) > 0);
        let kind = if colon_starts_qname {
            self.pos += 1; /* eat ':' */
            if self.at(0) == Some(b'*') {
                self.pos += 1;
            } else {
                loop {
                    let n = ncname_char(self.rest(), false);
                    if n == 0 {
                        break;
                    }
                    self.pos += n;
                }
            }
            Tok::QName
        } else {
            Tok::Name
        };
        Token {
            kind,
            off: start,
            len: self.pos - start,
            num: 0.0,
        }
    }
}
