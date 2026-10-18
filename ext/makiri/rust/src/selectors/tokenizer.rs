//! The CSS syntax tokenizer, ported from Lexbor's `css/syntax/tokenizer.c`,
//! `css/syntax/token.c` (the token queue) and `css/syntax/state.c`.
//!
//! Lexbor's shape is kept: a state is a function that reads one token from
//! `[data, end)` and returns where it stopped, chosen by the first byte through
//! `lxb_css_syntax_state_res_map` (here the `match` in [`Tokenizer::dispatch`]).
//! Each Rust state names the Lexbor function it ports. What changed, and only
//! for ownership:
//!
//! - **Positions are indices** into the input, and Lexbor's NULL return (out
//!   of memory) is `None`.
//! - **The temporary buffer is a `Vec<u8>`** (Lexbor's `start..pos`), and a
//!   token's string is copied out of it into one store for the whole input,
//!   where Lexbor copies it into its arena when the next token is read.
//! - **The token queue is a `Vec`** read from `head`: Lexbor's `first`..`last`
//!   list, which holds the tokens read ahead and not yet consumed.
//! - The parse-error list is not kept: nothing reads it.

use super::strtod::strtod_internal;
use super::token::{Span, Token, TokenType};
use super::Status;
use crate::falloc::{Reserve, VecPush};

/// `LXB_CSS_SYNTAX_ERROR_CODEPOINT`.
const ERROR_CODEPOINT: u32 = 0x1F_FFFF;

/// U+FFFD in UTF-8.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// `lxb_css_syntax_tokenizer_important`.
const IMPORTANT: &[u8] = b"important";

/// `LXB_CSS_SYNTAX_RES_NAME_START`.
const NAME_START: u8 = 0x01;

/// `lxb_css_syntax_res_name_map`: 1 for a name-start byte, 2 for one that
/// only continues a name, 0 otherwise. Lexbor initialises 255 of the 256
/// entries; C zero-fills the last.
static NAME_MAP: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 1,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0,
];

/// `lexbor_str_res_map_hex`: a hex digit's value, 0xFF for anything else.
#[inline]
fn hex(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0xFF,
    }
}

/// `lexbor_utils_whitespace(ch, ==, ||)`.
#[inline]
fn is_ws(ch: u8) -> bool {
    matches!(ch, b' ' | b'\t' | b'\n' | 0x0C | b'\r')
}

/// `lxb_css_syntax_state_non_ascii`.
fn non_ascii(cp: u32) -> bool {
    matches!(cp,
        0x00B7
        | 0x00C0..=0x00D6
        | 0x00D8..=0x00F6
        | 0x00F8..=0x037D
        | 0x037F..=0x1FFF
        | 0x200C..=0x200D
        | 0x203F..=0x2040
        | 0x2070..=0x218F
        | 0x2C00..=0x2FEF
        | 0x3001..=0xD7FF
        | 0xF900..=0xFDCF
        | 0xFDF0..=0xFFFD
        | 0x10000..=0x10FFFF)
}

/// The tokenizer over one input.
pub struct Tokenizer<'a> {
    input: &'a [u8],
    /// `in_p`: where the next token starts.
    in_p: usize,
    /// `offset`: the next token's offset.
    offset: usize,
    /// The temporary buffer, `start..pos`.
    buf: Vec<u8>,
    /// Every token's string, end to end.
    strings: Vec<u8>,
    /// The tokens read and not yet consumed, from `head`.
    queue: Vec<Token>,
    head: usize,
    pub with_comment: bool,
    pub with_unicode_range: bool,
    pub status: Status,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a [u8]) -> Tokenizer<'a> {
        Tokenizer {
            input,
            in_p: 0,
            offset: 0,
            buf: Vec::new(),
            strings: Vec::new(),
            queue: Vec::new(),
            head: 0,
            with_comment: false,
            with_unicode_range: false,
            status: Status::Ok,
        }
    }

    /// The bytes of a token's string.
    pub fn string(&self, s: Span) -> &[u8] {
        let start = s.start as usize;
        self.strings
            .get(start..start + s.len as usize)
            .unwrap_or(&[])
    }

    /// `lxb_css_syntax_token`: the first token not consumed, read if there is
    /// none.
    pub fn token(&mut self) -> Option<Token> {
        if let Some(t) = self.queue.get(self.head) {
            return Some(*t);
        }
        self.next_token()
    }

    /// `lxb_css_syntax_token_next`: read one more token past those queued.
    pub fn token_next(&mut self) -> Option<Token> {
        self.next_token()
    }

    /// `lxb_css_syntax_token_consume`.
    pub fn consume(&mut self) {
        if self.head < self.queue.len() {
            self.head += 1;
            if self.head == self.queue.len() {
                self.queue.clear();
                self.head = 0;
            }
        }
    }

    /// The token read ahead `n` past the current one (`first->next...`).
    fn ahead(&self, n: usize) -> Option<&Token> {
        if self.head >= self.queue.len() {
            return None;
        }
        self.queue.get(self.head + n)
    }

    /// `lxb_css_syntax_tokenizer_lookup_colon`: whether a `:` follows the
    /// current token, past whitespace. Read from the tokens read ahead when
    /// there are any, else from the input.
    pub fn lookup_colon(&self) -> bool {
        if let Some(token) = self.ahead(1) {
            if token.ty == TokenType::Whitespace {
                if let Some(next) = self.ahead(2) {
                    return next.ty == TokenType::Colon;
                }
            } else if token.ty == TokenType::Colon {
                return true;
            }
            return false;
        }
        let mut p = self.in_p;
        while p < self.input.len() {
            match self.input[p] {
                b':' => return true,
                b if is_ws(b) => p += 1,
                _ => return false,
            }
        }
        false
    }

    /// `lxb_css_syntax_tokenizer_lookup_important`: whether `important`
    /// follows the current `!` and ends the declaration.
    pub fn lookup_important(&self, stop: TokenType, stop_ch: u8) -> bool {
        let p = self.in_p;
        if let Some(token) = self.ahead(1) {
            if token.ty != TokenType::Ident
                || !self.string(token.string).eq_ignore_ascii_case(IMPORTANT)
            {
                return false;
            }
            let Some(mut token) = self.ahead(2) else {
                return self.important_end(p, stop_ch);
            };
            if token.ty == TokenType::Whitespace {
                match self.ahead(3) {
                    Some(next) => token = next,
                    None => return self.important_end(p, stop_ch),
                }
            }
            return token.ty == TokenType::Semicolon
                || token.ty == stop
                || token.ty == TokenType::Eof;
        }
        self.important_ch(p, stop_ch)
    }

    /// `lxb_css_syntax_tokenizer_lookup_important_ch`.
    fn important_ch(&self, p: usize, stop_ch: u8) -> bool {
        match self.input.get(p..p + IMPORTANT.len()) {
            Some(s) if s.eq_ignore_ascii_case(IMPORTANT) => {
                self.important_end(p + IMPORTANT.len(), stop_ch)
            }
            _ => false,
        }
    }

    /// `lxb_css_syntax_tokenizer_lookup_important_end`.
    fn important_end(&self, mut p: usize, stop_ch: u8) -> bool {
        while p < self.input.len() {
            match self.input[p] {
                b';' => return true,
                b if is_ws(b) => p += 1,
                b => return stop_ch != 0 && stop_ch == b,
            }
        }
        true
    }

    /// `lxb_css_syntax_tokenizer_lookup_declaration_ws_end`: whether the
    /// whitespace just read ends the value - a `;`, `!important` or the
    /// block's end follows.
    ///
    /// Lexbor's two quirks on the read-ahead path are kept: it looks for
    /// `important` after a delimiter that is NOT `!`, and it compares the
    /// token's type with the stop character.
    pub fn lookup_declaration_ws_end(&self, stop: TokenType, stop_ch: u8) -> bool {
        if let Some(token) = self.ahead(1) {
            return match token.ty {
                TokenType::Delim => {
                    if token.delim != u32::from(b'!') {
                        return self.lookup_important(stop, stop_ch);
                    }
                    false
                }
                TokenType::Semicolon => true,
                ty => ty as u32 == u32::from(stop_ch) || ty == TokenType::Eof,
            };
        }
        let p = self.in_p;
        match self.input.get(p) {
            Some(b';') => true,
            Some(b'!') => self.important_ch(p + 1, stop_ch),
            Some(&b) => stop_ch != 0 && stop_ch == b,
            None => false,
        }
    }

    fn oom<T>(&mut self) -> Option<T> {
        self.status = Status::ErrorMemoryAllocation;
        None
    }

    /// `lxb_css_syntax_tokenizer_token`.
    fn next_token(&mut self) -> Option<Token> {
        let end = self.input.len();
        let mut begin = self.in_p;
        let token = loop {
            let mut token = Token {
                begin,
                ..Token::default()
            };
            if begin < end {
                begin = self.dispatch(&mut token, begin, end)?;
            } else {
                token.ty = TokenType::Eof;
            }
            token.offset = self.offset;
            token.length = begin - self.in_p;
            self.offset += token.length;
            self.in_p = begin;
            if token.ty == TokenType::Comment && !self.with_comment {
                continue;
            }
            break token;
        };
        if self.queue.falloc_push(token).is_err() {
            return self.oom();
        }
        Some(token)
    }

    /// `lxb_css_syntax_state_res_map[*begin](...)`.
    fn dispatch(&mut self, token: &mut Token, data: usize, end: usize) -> Option<usize> {
        match self.input[data] {
            0x00 => self.ident_like_not_url(token, data, end),
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => self.whitespace(token, data, end),
            b'"' | b'\'' => self.string_state(token, data, end),
            b'#' => self.hash(token, data, end),
            b'(' => Some(simple(token, TokenType::LParenthesis, data)),
            b')' => Some(simple(token, TokenType::RParenthesis, data)),
            b'+' => self.plus(token, data, end),
            b',' => Some(simple(token, TokenType::Comma, data)),
            b'-' => self.minus(token, data, end),
            b'.' => self.full_stop(token, data, end),
            b'/' => self.comment(token, data, end),
            b'0'..=b'9' => {
                token.have_sign = false;
                self.consume_numeric(token, data, end)
            }
            b':' => Some(simple(token, TokenType::Colon, data)),
            b';' => Some(simple(token, TokenType::Semicolon, data)),
            b'<' => Some(less_sign(self.input, token, data, end)),
            b'@' => self.at(token, data, end),
            b'U' | b'u' => self.ident_like(token, data, end),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.ident_like_not_url(token, data, end),
            b'[' => Some(simple(token, TokenType::LsBracket, data)),
            b'\\' => self.rsolidus(token, data, end),
            b']' => Some(simple(token, TokenType::RsBracket, data)),
            b'{' => Some(simple(token, TokenType::LcBracket, data)),
            b'}' => Some(simple(token, TokenType::RcBracket, data)),
            0xED => self.ident_like_not_url_surrogate(token, data, end),
            0x80..=0xFF => self.ident_like_not_url_start(token, data, end),
            ch => Some(delim_set(token, data, ch)),
        }
    }

    /* ---- the temporary buffer ---- */

    /// `lxb_css_syntax_buffer_append_m` over input bytes.
    fn append(&mut self, from: usize, to: usize) -> Option<()> {
        if to > from {
            let bytes = &self.input[from..to];
            if self.buf.falloc_reserve(bytes.len()).is_err() {
                return self.oom();
            }
            self.buf.extend_from_slice(bytes);
        }
        Some(())
    }

    /// `lxb_css_syntax_buffer_append_m` over other bytes.
    fn append_bytes(&mut self, bytes: &[u8]) -> Option<()> {
        if self.buf.falloc_reserve(bytes.len()).is_err() {
            return self.oom();
        }
        self.buf.extend_from_slice(bytes);
        Some(())
    }

    /// `LXB_CSS_SYNTAX_CARRIAGE_RETURN_BLOCK`: returns the new `begin` and
    /// moves `data` onto the LF of a CR LF pair.
    fn cr_block(&mut self, begin: usize, data: &mut usize, end: usize) -> Option<usize> {
        self.append(begin, *data + 1)?;
        if let Some(last) = self.buf.last_mut() {
            *last = 0x0A;
        }
        if *data + 1 < end && self.input[*data + 1] == 0x0A {
            *data += 1;
        }
        Some(*data + 1)
    }

    /// `LXB_CSS_SYNTAX_FORM_FEED_BLOCK`.
    fn ff_block(&mut self, begin: usize, data: usize) -> Option<usize> {
        self.append(begin, data + 1)?;
        if let Some(last) = self.buf.last_mut() {
            *last = 0x0A;
        }
        Some(data + 1)
    }

    /// `LXB_CSS_SYNTAX_NULL_BLOCK`.
    fn null_block(&mut self, begin: usize, data: usize) -> Option<usize> {
        self.append(begin, data)?;
        self.append_bytes(REPLACEMENT)?;
        Some(data + 1)
    }

    /// `LXB_CSS_SYNTAX_UTF_8_UP_80_BLOCK`: decodes at `*data`, and on an
    /// ill-formed sequence flushes the run before it and writes U+FFFD.
    /// Returns the code point and the position the sequence started at.
    fn utf8_block(
        &mut self,
        begin: &mut usize,
        data: &mut usize,
        end: usize,
    ) -> Option<(u32, usize)> {
        let bad = *data;
        let cp = decode_utf_8_up_80(self.input, data, end);
        if cp == ERROR_CODEPOINT {
            self.append(*begin, bad)?;
            self.append_bytes(REPLACEMENT)?;
            *begin = *data;
        }
        Some((cp, bad))
    }

    /// `lxb_css_syntax_state_string_set`: the buffer becomes the token's
    /// string.
    fn string_set(&mut self, token: &mut Token, data: usize) -> Option<usize> {
        let start = self.strings.len();
        let Ok(start32) = u32::try_from(start) else {
            return self.oom();
        };
        let Ok(len32) = u32::try_from(self.buf.len()) else {
            return self.oom();
        };
        if self.strings.falloc_reserve(self.buf.len()).is_err() {
            return self.oom();
        }
        self.strings.extend_from_slice(&self.buf);
        self.buf.clear();
        token.string = Span {
            start: start32,
            len: len32,
        };
        Some(data)
    }

    /// `lxb_css_syntax_codepoint_to_ascii`.
    fn codepoint_to_ascii(&mut self, cp: u32) -> Option<()> {
        if cp == 0 || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
            return self.append_bytes(REPLACEMENT);
        }
        let mut b = [0u8; 4];
        let n = if cp <= 0x7F {
            b[0] = cp as u8;
            1
        } else if cp <= 0x7FF {
            b[0] = 0xC0 | (cp >> 6) as u8;
            b[1] = 0x80 | (cp & 0x3F) as u8;
            2
        } else if cp <= 0xFFFF {
            b[0] = 0xE0 | (cp >> 12) as u8;
            b[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            b[2] = 0x80 | (cp & 0x3F) as u8;
            3
        } else {
            b[0] = 0xF0 | (cp >> 18) as u8;
            b[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
            b[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            b[3] = 0x80 | (cp & 0x3F) as u8;
            4
        };
        self.append_bytes(&b[..n])
    }

    /* ---- the states ---- */

    /// `lxb_css_syntax_state_comment`.
    fn comment(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        /* Skip forward slash (/) */
        data += 1;
        if data >= end || self.input[data] != b'*' {
            return Some(delim_set(token, data - 1, b'/'));
        }
        data += 1;
        let mut begin = data;
        let mut done = false;
        while data < end {
            match self.input[data] {
                b'*' => {
                    data += 1;
                    if data >= end {
                        self.append(begin, data)?;
                        done = true;
                        break;
                    }
                    if self.input[data] == b'/' {
                        self.append(begin, data - 1)?;
                        data += 1;
                        done = true;
                        break;
                    }
                    continue;
                }
                0x0D => begin = self.cr_block(begin, &mut data, end)?,
                0x0C => begin = self.ff_block(begin, data)?,
                0x00 => begin = self.null_block(begin, data)?,
                ch => {
                    if ch >= 0x80 {
                        self.utf8_block(&mut begin, &mut data, end)?;
                        continue;
                    }
                }
            }
            data += 1;
        }
        if !done {
            self.append(begin, data)?;
        }
        token.ty = TokenType::Comment;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_whitespace`.
    fn whitespace(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        let mut begin = data;
        loop {
            match self.input[data] {
                0x0D => begin = self.cr_block(begin, &mut data, end)?,
                0x0C => begin = self.ff_block(begin, data)?,
                0x09 | 0x20 | 0x0A => {}
                _ => break,
            }
            data += 1;
            if data >= end {
                break;
            }
        }
        token.ty = TokenType::Whitespace;
        self.append(begin, data)?;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_string`.
    fn string_state(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        let mark = self.input[data];
        data += 1;
        let mut begin = data;
        token.ty = TokenType::String;
        while data < end {
            match self.input[data] {
                0x00 => begin = self.null_block(begin, data)?,
                0x0A | 0x0D | 0x0C => {
                    token.ty = TokenType::BadString;
                    self.append(begin, data)?;
                    return self.string_set(token, data);
                }
                b'\\' => {
                    self.append(begin, data)?;
                    data += 1;
                    begin = data;
                    if data >= end {
                        data = self.escaped(data, end)?;
                        begin = data;
                        continue;
                    }
                    match self.input[data] {
                        0x0D => {
                            if data + 1 < end && self.input[data + 1] == 0x0A {
                                data += 1;
                            }
                            begin = data + 1;
                        }
                        0x0A | 0x0C => begin = data + 1,
                        0x00 => begin = self.null_block(begin, data)?,
                        ch => {
                            if ch < 0x80 {
                                data = self.escaped(data, end)?;
                                begin = data;
                            } else {
                                let cp = decode_utf_8_up_80(self.input, &mut data, end);
                                if cp == ERROR_CODEPOINT {
                                    self.append_bytes(REPLACEMENT)?;
                                    begin = data;
                                }
                            }
                            continue;
                        }
                    }
                }
                ch => {
                    if ch == mark {
                        self.append(begin, data)?;
                        return self.string_set(token, data + 1);
                    }
                    if ch >= 0x80 {
                        self.utf8_block(&mut begin, &mut data, end)?;
                        continue;
                    }
                }
            }
            data += 1;
        }
        self.append(begin, data)?;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_hash`.
    fn hash(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        data += 1;
        if data >= end {
            return Some(delim_set(token, data - 1, b'#'));
        }
        let ch = self.input[data];
        if ch >= 0x80 {
            let begin = data;
            if start_ident_utf_8_80(self.input, &mut data, end) {
                data = begin;
            } else {
                return Some(delim_set(token, begin - 1, b'#'));
            }
        } else if NAME_MAP[ch as usize] == 0x00 && ch != 0x00 {
            if ch != b'\\' {
                return Some(delim_set(token, data - 1, b'#'));
            }
            if data + 1 < end && matches!(self.input[data + 1], 0x0A | 0x0C | 0x0D) {
                return Some(delim_set(token, data - 1, b'#'));
            }
        }
        token.ty = TokenType::Hash;
        self.consume_ident(token, data, end)
    }

    /// `lxb_css_syntax_state_plus`.
    fn plus(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        data += 1;
        if data >= end {
            return Some(delim_set(token, data - 1, b'+'));
        }
        if self.input[data].is_ascii_digit() {
            token.have_sign = true;
            return self.consume_numeric(token, data, end);
        }
        if self.input[data] == b'.' {
            data += 1;
            if data >= end {
                return Some(delim_set(token, data - 2, b'+'));
            }
            if self.input[data].is_ascii_digit() {
                token.have_sign = true;
                return self.consume_numeric(token, data - 1, end);
            }
            data -= 1;
        }
        Some(delim_set(token, data - 1, b'+'))
    }

    /// `lxb_css_syntax_state_minus`.
    fn minus(&mut self, token: &mut Token, data: usize, end: usize) -> Option<usize> {
        if start_number(self.input, data, end) {
            let data = self.consume_numeric(token, data + 1, end)?;
            token.num = -token.num;
            token.have_sign = true;
            return Some(data);
        }
        if data + 3 <= end && self.input[data + 1] == b'-' && self.input[data + 2] == b'>' {
            token.ty = TokenType::Cdc;
            return Some(data + 3);
        }
        if start_ident_sequence(self.input, data, end) {
            return self.ident_like_not_url(token, data, end);
        }
        Some(delim_set(token, data, b'-'))
    }

    /// `lxb_css_syntax_state_full_stop`.
    fn full_stop(&mut self, token: &mut Token, data: usize, end: usize) -> Option<usize> {
        if start_number(self.input, data, end) {
            token.have_sign = false;
            return self.consume_numeric(token, data, end);
        }
        Some(delim_set(token, data, b'.'))
    }

    /// `lxb_css_syntax_state_at`.
    fn at(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        data += 1;
        if start_ident_sequence(self.input, data, end) {
            token.ty = TokenType::AtKeyword;
            return self.consume_ident(token, data, end);
        }
        Some(delim_set(token, data - 1, b'@'))
    }

    /// `lxb_css_syntax_state_rsolidus`.
    fn rsolidus(&mut self, token: &mut Token, data: usize, end: usize) -> Option<usize> {
        if valid_escape(self.input, data, end) {
            return self.ident_like(token, data, end);
        }
        Some(delim_set(token, data, b'\\'))
    }

    /// `lxb_css_syntax_consume_numeric_set`.
    fn numeric_set(
        token: &mut Token,
        digits: &[u8],
        is_float: bool,
        e_is_negative: bool,
        exponent: i32,
        e_digit: i32,
    ) {
        let exponent = if e_is_negative {
            -(e_digit - exponent)
        } else {
            e_digit + exponent
        };
        token.ty = TokenType::Number;
        token.is_float = is_float;
        token.num = strtod_internal(digits, exponent);
    }

    /// `lxb_css_syntax_state_consume_numeric`.
    fn consume_numeric(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        /* Lexbor reads the digits into a 128-byte buffer and drops the rest. */
        let mut buf = [0u8; 128];
        let mut n = 0;
        let input = self.input;

        while input[data].is_ascii_digit() {
            if n != buf.len() {
                buf[n] = input[data];
                n += 1;
            }
            data += 1;
            if data >= end {
                Self::numeric_set(token, &buf[..n], false, false, 0, 0);
                return Some(data);
            }
        }

        let mut exponent = 0;
        let mut is_float = false;

        if input[data] == b'.' {
            data += 1;
            if data >= end || !input[data].is_ascii_digit() {
                Self::numeric_set(token, &buf[..n], false, false, 0, 0);
                return Some(data - 1);
            }
            let begin = n;
            loop {
                if n != buf.len() {
                    buf[n] = input[data];
                    n += 1;
                }
                data += 1;
                if !(data < end && input[data].is_ascii_digit()) {
                    break;
                }
            }
            exponent = -((n - begin) as i32);
            is_float = true;
            if data >= end {
                Self::numeric_set(token, &buf[..n], true, false, exponent, 0);
                return Some(data);
            }
        }

        let ch = input[data];
        if ch != b'E' && ch != b'e' {
            Self::numeric_set(token, &buf[..n], is_float, false, exponent, 0);
            return self.consume_numeric_name_start(token, data, end);
        }

        data += 1;
        if data >= end {
            data -= 1;
            self.append(data, data + 1)?;
            Self::numeric_set(token, &buf[..n], is_float, false, exponent, 0);
            token.ty = TokenType::Dimension;
            return self.string_set(token, data + 1);
        }

        let mut e_is_negative = false;
        match input[data] {
            b'-' => {
                e_is_negative = true;
                data += 1;
            }
            b'+' => data += 1,
            _ => {}
        }

        if data >= end || !input[data].is_ascii_digit() {
            data -= 1;
            if input[data] != ch {
                data -= 1;
            }
            Self::numeric_set(token, &buf[..n], is_float, false, exponent, 0);
            token.ty = TokenType::Dimension;
            return self.consume_ident(token, data, end);
        }

        let mut e_digit: i32 = 0;
        loop {
            if e_digit < i32::MAX / 10 {
                e_digit = i32::from(input[data] - b'0') + e_digit * 10;
            }
            data += 1;
            if data >= end {
                Self::numeric_set(token, &buf[..n], true, e_is_negative, exponent, e_digit);
                return Some(data);
            }
            if !input[data].is_ascii_digit() {
                break;
            }
        }

        Self::numeric_set(token, &buf[..n], true, e_is_negative, exponent, e_digit);
        self.consume_numeric_name_start(token, data, end)
    }

    /// `lxb_css_syntax_state_consume_numeric_name_start`.
    fn consume_numeric_name_start(
        &mut self,
        token: &mut Token,
        data: usize,
        end: usize,
    ) -> Option<usize> {
        if start_ident_sequence(self.input, data, end) {
            token.ty = TokenType::Dimension;
            return self.consume_ident(token, data, end);
        }
        if self.input[data] == b'%' {
            token.ty = TokenType::Percentage;
            return Some(data + 1);
        }
        Some(data)
    }

    /// `lxb_css_syntax_state_consume_ident`.
    fn consume_ident(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        let mut begin = data;
        while data < end {
            let ch = self.input[data];
            if ch < 0x80 {
                if NAME_MAP[ch as usize] == 0x00 {
                    if ch == b'\\' {
                        self.append(begin, data)?;
                        data += 1;
                        if data < end && matches!(self.input[data], 0x0A | 0x0C | 0x0D) {
                            data -= 1;
                            begin = data;
                            break;
                        }
                        data = self.escaped(data, end)?;
                        begin = data;
                        continue;
                    } else if ch == 0x00 {
                        begin = self.null_block(begin, data)?;
                    } else {
                        break;
                    }
                }
                data += 1;
            } else {
                let (cp, bad) = self.utf8_block(&mut begin, &mut data, end)?;
                if cp != ERROR_CODEPOINT && !non_ascii(cp) {
                    data = bad;
                    break;
                }
            }
        }
        self.append(begin, data)?;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_consume_unicode_range`.
    fn consume_unicode_range(
        &mut self,
        token: &mut Token,
        mut data: usize,
        end: usize,
    ) -> Option<usize> {
        token.ty = TokenType::UnicodeRange;
        data += 2;
        let mut mq = true;
        loop {
            let mut cp: u32 = 0;
            let mut count = 6;
            while data < end && count > 0 {
                let h = hex(self.input[data]);
                if h == 0xFF {
                    if self.input[data] == b'?' && mq {
                        let mut question = 0;
                        loop {
                            question += 1;
                            count -= 1;
                            data += 1;
                            if !(data < end && self.input[data] == b'?' && count > 0) {
                                break;
                            }
                        }
                        let range_start = cp << (4 * question);
                        token.range_start = range_start;
                        token.range_end = range_start | ((1u32 << (4 * question)) - 1);
                        return Some(data);
                    }
                    break;
                }
                cp = (cp << 4) | u32::from(h);
                count -= 1;
                data += 1;
            }
            if mq {
                mq = false;
                token.range_start = cp;
                if data + 2 > end || self.input[data] != b'-' || hex(self.input[data + 1]) == 0xFF {
                    token.range_end = cp;
                    return Some(data);
                }
                data += 1;
                continue;
            }
            token.range_end = cp;
            return Some(data);
        }
    }

    /// `lxb_css_syntax_state_ident_like`.
    fn ident_like(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        if self.with_unicode_range
            && data + 3 <= end
            && self.input[data + 1] == b'+'
            && (hex(self.input[data + 2]) != 0xFF || self.input[data + 2] == b'?')
        {
            return self.consume_unicode_range(token, data, end);
        }
        data = self.consume_ident(token, data, end)?;
        if data < end && self.input[data] == b'(' {
            data += 1;
            if !self.string(token.string).eq_ignore_ascii_case(b"url") {
                token.ty = TokenType::Function;
                return Some(data);
            }
            let begin = data;
            while data < end {
                let ch = self.input[data];
                if !is_ws(ch) {
                    if ch == b'"' || ch == b'\'' {
                        break;
                    }
                    self.buf.clear();
                    return self.url(token, data, end);
                }
                data += 1;
            }
            token.ty = TokenType::Function;
            return Some(begin);
        }
        token.ty = TokenType::Ident;
        Some(data)
    }

    /// `lxb_css_syntax_state_ident_like_not_url_start`.
    fn ident_like_not_url_start(
        &mut self,
        token: &mut Token,
        data: usize,
        end: usize,
    ) -> Option<usize> {
        let mut p = data;
        if start_ident_utf_8_80(self.input, &mut p, end) {
            return self.ident_like_not_url(token, data, end);
        }
        Some(delim_set_cp(self.input, token, data, end))
    }

    /// `lxb_css_syntax_state_ident_like_not_url`.
    fn ident_like_not_url(&mut self, token: &mut Token, data: usize, end: usize) -> Option<usize> {
        let data = self.consume_ident(token, data, end)?;
        if data < end && self.input[data] == b'(' {
            token.ty = TokenType::Function;
            return Some(data + 1);
        }
        token.ty = TokenType::Ident;
        Some(data)
    }

    /// `lxb_css_syntax_state_ident_like_not_url_surrogate`.
    fn ident_like_not_url_surrogate(
        &mut self,
        token: &mut Token,
        data: usize,
        end: usize,
    ) -> Option<usize> {
        if data + 3 <= end {
            let srp = u32::from(self.input[data + 1]) << 8 | u32::from(self.input[data + 2]);
            if ((0xA0 << 8 | 0x80)..=(0xBF << 8 | 0xBF)).contains(&srp) {
                return self.ident_like_not_url(token, data, end);
            }
        }
        let mut p = data;
        if start_ident_utf_8_80(self.input, &mut p, end) {
            return self.ident_like_not_url(token, data, end);
        }
        Some(delim_set_cp(self.input, token, data, end))
    }

    /// `lxb_css_syntax_state_url`.
    fn url(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        let mut begin = data;
        let mut bad = false;
        let mut eof = true;
        while data < end {
            match self.input[data] {
                0x00 => begin = self.null_block(begin, data)?,
                b')' => {
                    self.append(begin, data)?;
                    data += 1;
                    eof = false;
                    break;
                }
                b'"' | b'\'' | b'(' | 0x0B | 0x7F => {
                    bad = true;
                    break;
                }
                b'\\' => {
                    self.append(begin, data)?;
                    data += 1;
                    if data < end && matches!(self.input[data], 0x0A | 0x0C | 0x0D) {
                        return self.bad_url(token, data, end);
                    }
                    data = self.escaped(data, end)?;
                    begin = data;
                    continue;
                }
                0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
                    self.append(begin, data)?;
                    data += 1;
                    while data < end {
                        let ch = self.input[data];
                        if !is_ws(ch) {
                            if ch == b')' {
                                data += 1;
                                token.ty = TokenType::Url;
                                return self.string_set(token, data);
                            }
                            return self.bad_url(token, data, end);
                        }
                        data += 1;
                    }
                    token.ty = TokenType::Url;
                    return self.string_set(token, data);
                }
                ch => {
                    if ch >= 0x80 {
                        self.utf8_block(&mut begin, &mut data, end)?;
                        continue;
                    }
                    if ch <= 0x08 || (0x0E..=0x1F).contains(&ch) {
                        bad = true;
                        break;
                    }
                }
            }
            data += 1;
        }
        if bad {
            self.append(begin, data)?;
            return self.bad_url(token, data, end);
        }
        if eof {
            self.append(begin, data)?;
        }
        token.ty = TokenType::Url;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_bad_url`.
    fn bad_url(&mut self, token: &mut Token, mut data: usize, end: usize) -> Option<usize> {
        token.ty = TokenType::BadUrl;
        let mut begin = data;
        while data < end {
            if self.input[data] == b')' {
                self.append(begin, data)?;
                return self.string_set(token, data + 1);
            } else if self.input[data] == b'\\' {
                data += 1;
                if data >= end {
                    break;
                }
            }
            if self.input[data] >= 0x80 {
                self.utf8_block(&mut begin, &mut data, end)?;
                continue;
            }
            data += 1;
        }
        self.append(begin, data)?;
        self.string_set(token, data)
    }

    /// `lxb_css_syntax_state_escaped`: reads the escape after a backslash
    /// and writes the code point it names.
    fn escaped(&mut self, mut data: usize, end: usize) -> Option<usize> {
        let mut cp: u32 = 0;
        let mut count = 0;
        let mut hexed = true;
        while count < 6 {
            if data >= end {
                hexed = false;
                break;
            }
            let h = hex(self.input[data]);
            if h == 0xFF {
                if count == 0 {
                    if self.input[data] < 0x80 {
                        cp = u32::from(self.input[data]);
                        data += 1;
                    } else {
                        cp = decode_utf_8_up_80(self.input, &mut data, end);
                    }
                    hexed = false;
                }
                break;
            }
            cp = (cp << 4) | u32::from(h);
            count += 1;
            data += 1;
        }
        if hexed && data < end {
            match self.input[data] {
                0x0D => {
                    data += 1;
                    if data < end && self.input[data] == 0x0A {
                        data += 1;
                    }
                }
                0x09 | 0x20 | 0x0A | 0x0C => data += 1,
                _ => {}
            }
        }
        self.codepoint_to_ascii(cp)?;
        Some(data)
    }
}

/// A one-byte token of type `ty`.
fn simple(token: &mut Token, ty: TokenType, data: usize) -> usize {
    token.ty = ty;
    data + 1
}

/// `lxb_css_syntax_state_delim_set`.
fn delim_set(token: &mut Token, data: usize, ch: u8) -> usize {
    token.ty = TokenType::Delim;
    token.delim = u32::from(ch);
    data + 1
}

/// `lxb_css_syntax_state_delim_set_cp`.
fn delim_set_cp(input: &[u8], token: &mut Token, mut data: usize, end: usize) -> usize {
    let cp = if input[data] < 0x80 {
        data += 1;
        u32::from(input[data - 1])
    } else {
        decode_utf_8_up_80(input, &mut data, end)
    };
    token.ty = TokenType::Delim;
    token.delim = cp;
    data
}

/// `lxb_css_syntax_state_less_sign`.
fn less_sign(input: &[u8], token: &mut Token, data: usize, end: usize) -> usize {
    if data + 4 <= end
        && input[data + 1] == b'!'
        && input[data + 2] == b'-'
        && input[data + 3] == b'-'
    {
        token.ty = TokenType::Cdo;
        return data + 4;
    }
    delim_set(token, data, b'<')
}

/// `lxb_css_syntax_state_valid_escape`.
fn valid_escape(input: &[u8], data: usize, end: usize) -> bool {
    if input[data] == b'\\' {
        let next = data + 1;
        if next >= end || !matches!(input[next], 0x0A | 0x0C | 0x0D) {
            return true;
        }
    }
    false
}

/// `lxb_css_syntax_state_start_ident_utf_8_80`.
fn start_ident_utf_8_80(input: &[u8], data: &mut usize, end: usize) -> bool {
    let cp = decode_utf_8_up_80(input, data, end);
    if cp == ERROR_CODEPOINT {
        return true;
    }
    non_ascii(cp)
}

/// `lxb_css_syntax_state_start_number`.
fn start_number(input: &[u8], mut data: usize, end: usize) -> bool {
    if matches!(input[data], b'+' | b'-') {
        data += 1;
        if data >= end {
            return false;
        }
        if input[data].is_ascii_digit() {
            return true;
        }
    }
    if input[data].is_ascii_digit() {
        return true;
    }
    if input[data] == b'.' {
        data += 1;
        return data < end && input[data].is_ascii_digit();
    }
    false
}

/// `lxb_css_syntax_state_start_ident_sequence`.
fn start_ident_sequence(input: &[u8], mut data: usize, end: usize) -> bool {
    if data >= end {
        return false;
    }
    let ch = input[data];
    if ch < 0x80 {
        if ch == b'-' {
            data += 1;
            if data >= end {
                return false;
            }
            let ch = input[data];
            if ch < 0x80
                && (ch == b'-'
                    || NAME_MAP[ch as usize] == NAME_START
                    || valid_escape(input, data, end)
                    || ch == 0x00)
            {
                return true;
            }
            /* Lexbor then reads the byte after the hyphen through the name
             * table, a byte >= 0x80 included. */
            return NAME_MAP[ch as usize] == NAME_START || ch == 0x00;
        } else if ch == b'\\' {
            return valid_escape(input, data, end);
        }
        return NAME_MAP[ch as usize] == NAME_START || ch == 0x00;
    }
    start_ident_utf_8_80(input, &mut data, end)
}

/// `lxb_css_syntax_state_decode_utf_8_up_80`: the WHATWG UTF-8 decoder over
/// one sequence starting at a byte >= 0x80. An ill-formed sequence is
/// `ERROR_CODEPOINT`, with `*data` past its maximal subpart.
fn decode_utf_8_up_80(input: &[u8], data: &mut usize, end: usize) -> u32 {
    let mut p = *data;
    let ch = input[p];
    p += 1;
    let (mut needed, mut lower, mut upper, mut cp);
    if ch <= 0xDF {
        if ch < 0xC2 {
            *data = p;
            return ERROR_CODEPOINT;
        }
        needed = 1;
        lower = 0x80;
        upper = 0xBF;
        cp = u32::from(ch & 0x1F);
    } else if ch <= 0xEF {
        needed = 2;
        lower = if ch == 0xE0 { 0xA0 } else { 0x80 };
        upper = if ch == 0xED { 0x9F } else { 0xBF };
        cp = u32::from(ch & 0x0F);
    } else if ch <= 0xF4 {
        needed = 3;
        lower = if ch == 0xF0 { 0x90 } else { 0x80 };
        upper = if ch == 0xF4 { 0x8F } else { 0xBF };
        cp = u32::from(ch & 0x07);
    } else {
        *data = p;
        return ERROR_CODEPOINT;
    }
    while needed != 0 {
        if p >= end || input[p] < lower || input[p] > upper {
            *data = p;
            return ERROR_CODEPOINT;
        }
        cp = (cp << 6) | u32::from(input[p] & 0x3F);
        p += 1;
        lower = 0x80;
        upper = 0xBF;
        needed -= 1;
    }
    *data = p;
    cp
}
