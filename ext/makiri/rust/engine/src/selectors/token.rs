//! `lxb_css_syntax_token_t` and its type, from `css/syntax/token.h`.

/// `lxb_css_syntax_token_type_t`, in Lexbor's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TokenType {
    #[default]
    Undef,
    Ident,
    Function,
    AtKeyword,
    Hash,
    String,
    BadString,
    Url,
    BadUrl,
    Comment,
    Whitespace,
    Dimension,
    Delim,
    UnicodeRange,
    Number,
    Percentage,
    Cdo,
    Cdc,
    Colon,
    Semicolon,
    Comma,
    LsBracket,
    RsBracket,
    LParenthesis,
    RParenthesis,
    LcBracket,
    RcBracket,
    Eof,
    /// `LXB_CSS_SYNTAX_TOKEN__END`: the parser's own end of a rule, never
    /// produced by the tokenizer.
    End,
}

/// A run of bytes in the tokenizer's string store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

/// One token. Lexbor's is a union over the token kinds; the fields here are
/// that union's members side by side, each meaningful for the kinds Lexbor
/// writes it for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Token {
    pub ty: TokenType,
    /// `token->offset`: where the token starts, counting from the start of
    /// the input. A comment the tokenizer drops still counts, so this is
    /// `begin` for the tokenizer that read the whole input.
    pub offset: usize,
    /// `base.begin`, an index into the input.
    pub begin: usize,
    /// `base.length`: how many input bytes the token was read from.
    pub length: usize,
    /// The string of an ident, function, at-keyword, hash, string, url,
    /// comment or whitespace; a dimension's unit.
    pub string: Span,
    /// A number, percentage or dimension's value.
    pub num: f64,
    pub is_float: bool,
    pub have_sign: bool,
    /// A delim's character.
    pub delim: u32,
    /// A unicode-range's bounds.
    pub range_start: u32,
    pub range_end: u32,
}
