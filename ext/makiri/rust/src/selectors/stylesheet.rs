//! The stylesheet reader, phase one of
//! `Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array`: the port of
//! Lexbor's stylesheet parser (`crate::selectors::parse_stylesheet`) walked
//! into owned Rust values. Phase two, turning them into Ruby, is
//! `glue::stylesheet`; this module knows nothing of Ruby, and nothing of
//! Lexbor any more (the name is the API's).
//!
//! A deliberately thin binding, returning plain Ruby primitives so the
//! abstraction seam lives in the caller (dommy's `internal/css/parser.rb`), not
//! here:
//!
//! ```text
//! [ { type: :style,
//!     selectors:    [ { text: "div.a", specificity: [a, b, c] }, ... ],
//!     declarations: [ { name: "display", value: "flex", important: false }, ... ] },
//!   { type: :bad_style, selector_text: "p::before", declarations: [ ... ] },
//!   { type: :at_rule, name: "media", prelude: "(min-width: 600px)", rules: [ ... ],
//!     declarations: [ ... ] } ]
//! ```
//!
//! The structure is Lexbor's: which rules there are, an at-rule's name and
//! prelude, a selector list Lexbor rejects surfacing as `:bad_style` with its
//! raw prelude for the caller to re-validate (as does one `selector_text`
//! cannot write back faithfully), where a declaration ends and whether it is
//! `!important`. A broken stylesheet never raises; only running out of memory
//! does, or a selector Lexbor's parser stops on outright.
//!
//! Names and values are not Lexbor's per-property normalization
//! (docs/rewrite_parser.ja.md, "stylesheet パーサ"): a declaration name is
//! ASCII-lower-cased unless it is a custom property (`--*`), an at-rule name
//! is lower-cased, and a value is the text between Lexbor's offsets with each
//! run of whitespace tokens written as one space and none at either end -
//! every other token (strings, urls, escapes, comments) as written.
//!
//! # Two phases, and why
//!
//! Phase two builds Ruby objects, and a Ruby raise `longjmp`s past `Drop`.
//! Phase one builds everything it needs first, so nothing that can raise
//! runs while anything needs freeing. Stylesheet parsing is rare by design -
//! once per `<style>`, not a hot path - so the intermediate is not worth
//! optimising away.

#![forbid(unsafe_code)]

use crate::compat::{self, Fix};
use crate::falloc::{self, OomOption, OomResult, Reserve, VecGrow};
use crate::selectors::ast::ListId;
use crate::selectors::rules::{AtKind, Item, ItemId};
use crate::selectors::specificity::unpack;
use crate::selectors::text;
use crate::selectors::token::TokenType;
use crate::selectors::tokenizer::Tokenizer;
use crate::selectors::{Sheet, Status};
use crate::stack::{Depth, TooDeep};

/// Bound on rule nesting in the walk: fail closed rather than recurse without
/// limit on a pathologically nested stylesheet.
pub const MAX_DEPTH: u32 = 64;

pub struct Decl {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    pub important: bool,
}

pub struct Selector {
    pub text: Vec<u8>,
    /// `[a, b, c]` per Selectors L4 §17.
    pub specificity: [u32; 3],
}

pub enum Rule {
    Style {
        selectors: Vec<Selector>,
        declarations: Vec<Decl>,
    },
    BadStyle {
        selector_text: Vec<u8>,
        declarations: Vec<Decl>,
    },
    At {
        name: Vec<u8>,
        prelude: Vec<u8>,
        rules: Vec<Rule>,
        /// The declarations in its block (`@font-face`, `@page`), every
        /// run of them in order (C16).
        declarations: Vec<Decl>,
    },
}

/// Anything that stops phase one. There is no syntax variant: the parser
/// recovers from syntax errors and this layer surfaces what it recovered.
pub enum Fail {
    Oom,
    TooDeep,
    /// The native stack ran low before [`MAX_DEPTH`] ([`crate::stack::Depth`]).
    StackLow,
    /// The parser stopped (a state Lexbor's parser stops the whole parse in).
    Parse,
}

impl crate::falloc::Oom for Fail {
    #[inline]
    fn oom() -> Self {
        Fail::Oom
    }
}

struct Conv<'a> {
    css: &'a [u8],
    sheet: &'a Sheet,
}

impl Conv<'_> {
    fn name(&self, span: crate::selectors::token::Span) -> &[u8] {
        let start = span.start as usize;
        self.sheet
            .ast
            .strings
            .get(start..start + span.len as usize)
            .unwrap_or(&[])
    }
}

/// A declaration name: lower-cased, unless it is a custom property.
fn declaration_name(name: &[u8]) -> Result<Vec<u8>, Fail> {
    let mut out = falloc::try_to_vec(name).or_oom()?;
    if !out.starts_with(b"--") {
        out.make_ascii_lowercase();
    }
    Ok(out)
}

/// A declaration value: `[begin, end)` of the text read as CSS tokens, each
/// run of whitespace tokens one space and none at either end, every other
/// token's text as written. Empty when the offsets are unusable - fail
/// closed, never a wrong slice.
pub fn normalize_value(css: &[u8], begin: usize, end: usize) -> Result<Vec<u8>, Fail> {
    let Some(src) = css.get(begin..end) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut tkz = Tokenizer::new(src);
    tkz.with_comment = true;
    let mut space = false;
    /* A `\` delim is a backslash before a newline, and it must stay before
     * one: before a space it would read back as an escape (C15). The
     * newline written after it stands for the whitespace that follows. */
    let mut after_backslash = false;
    loop {
        let Some(token) = tkz.token() else {
            return Err(match tkz.status {
                Status::ErrorMemoryAllocation => Fail::Oom,
                _ => Fail::Parse,
            });
        };
        tkz.consume();
        match token.ty {
            TokenType::Eof => break,
            TokenType::Whitespace if after_backslash => after_backslash = false,
            TokenType::Whitespace => space = !out.is_empty(),
            _ => {
                let text = src
                    .get(token.offset..token.offset + token.length)
                    .unwrap_or(&[]);
                out.falloc_reserve(text.len() + 2).or_oom()?;
                if space {
                    out.falloc_push(b' ').or_oom()?;
                    space = false;
                }
                out.falloc_extend_from_slice(text).or_oom()?;
                after_backslash = token.ty == TokenType::Delim
                    && token.delim == u32::from(b'\\')
                    && compat::fixed(Fix::C15);
                if after_backslash {
                    out.falloc_push(b'\n').or_oom()?;
                }
            }
        }
    }
    /* The value's range ends before the whitespace after it, so a backslash
     * that closes it (one not escaped by another) read as an escape at its
     * end; in the text it was a delim before a newline. */
    let trailing = out.iter().rev().take_while(|&&b| b == b'\\').count();
    if trailing % 2 == 1
        && !after_backslash
        && matches!(css.get(end), Some(b'\n' | b'\r' | 0x0C))
        && compat::fixed(Fix::C15)
    {
        out.falloc_push(b'\n').or_oom()?;
    }
    Ok(out)
}

fn declarations(c: &Conv, list: Option<ItemId>) -> Result<Vec<Decl>, Fail> {
    let mut out = Vec::new();
    for (_, item) in c.sheet.rules.children(list) {
        if let Item::Declaration {
            name,
            value_begin,
            value_end,
            important,
        } = *item
        {
            out.falloc_push(Decl {
                name: declaration_name(c.name(name))?,
                value: normalize_value(c.css, value_begin, value_end)?,
                important,
            })
            .or_oom()?;
        }
    }
    Ok(out)
}

/// Each comma alternative of a style rule's selector list, as text
/// (`selectors::text`) with its specificity - or None when one of them
/// cannot be written back as it was written, for the caller to report the
/// rule as `:bad_style` rather than hand out a selector that means something
/// else.
fn selectors(c: &Conv, first: Option<ListId>) -> Result<Option<Vec<Selector>>, Fail> {
    let mut out = Vec::new();
    for list in c.sheet.ast.lists_from(first) {
        let mut text = Vec::new();
        match text::write(list, &mut text) {
            Ok(()) => {}
            Err(text::Fail::Lossy) => return Ok(None),
            Err(text::Fail::Oom) => return Err(Fail::Oom),
        }
        out.falloc_push(Selector {
            text,
            specificity: unpack(list.specificity()),
        })
        .or_oom()?;
    }
    Ok(Some(out))
}

/// Slice `[begin, end)` out of the input, trimmed (`trim`) - the one copy.
/// Empty when the offsets are unusable - fail closed, never a wrong slice.
fn slice_trim(css: &[u8], begin: usize, end: usize) -> Result<Vec<u8>, Fail> {
    match css.get(begin..end) {
        Some(s) => falloc::try_to_vec(trim(s)).or_oom(),
        None => Ok(Vec::new()),
    }
}

/// `s` without its leading and trailing CSS whitespace - space, tab, LF, CR
/// and FF, nothing else (css-syntax-3; VT and the other controls are not
/// whitespace there). A trailing whitespace byte that an odd run of
/// backslashes precedes is kept, with everything before it: `.a\ ` is the
/// escaped space, and even where the pair is not an escape (`\` + LF) a `\`
/// left at the end would be one - an escape at EOF, which reads as U+FFFD, so
/// a prelude the parser rejected would come back as a different, valid
/// selector. (Trimming the space that ends a hex escape, `\31 `, leaves the
/// escape at the end, where it means the same.)
fn trim(mut s: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = s.split_first() {
        if !is_css_whitespace(b) {
            break;
        }
        s = rest;
    }
    while let Some((&b, rest)) = s.split_last() {
        if !is_css_whitespace(b) || escapes_next(rest) {
            break;
        }
        s = rest;
    }
    s
}

fn is_css_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

/// Whether `before` ends in an odd run of backslashes: one that escapes the
/// byte after it.
fn escapes_next(before: &[u8]) -> bool {
    before.iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

fn rules(c: &Conv, list: Option<ItemId>, depth: Depth) -> Result<Vec<Rule>, Fail> {
    let inner = depth.enter().map_err(|t| match t {
        TooDeep::Limit => Fail::TooDeep,
        TooDeep::StackLow => Fail::StackLow,
    })?;
    let mut out = Vec::new();
    for (_, item) in c.sheet.rules.children(list) {
        let entry = match *item {
            Item::Style {
                selector,
                prelude_begin,
                prelude_end,
                declarations: decls,
                ..
            } => {
                let declarations = declarations(c, decls)?;
                Some(match selectors(c, selector)? {
                    Some(selectors) => Rule::Style {
                        selectors,
                        declarations,
                    },
                    // Not writable as text: the caller re-validates the
                    // prelude as written, as for a selector the parser
                    // rejected.
                    None => Rule::BadStyle {
                        selector_text: slice_trim(c.css, prelude_begin, prelude_end)?,
                        declarations,
                    },
                })
            }
            Item::BadStyle {
                prelude_begin,
                prelude_end,
                declarations: decls,
                ..
            } => Some(Rule::BadStyle {
                selector_text: slice_trim(c.css, prelude_begin, prelude_end)?,
                declarations: declarations(c, decls)?,
            }),
            Item::At {
                kind,
                name,
                prelude_begin,
                prelude_end,
                block,
            } => {
                // An at-rule Lexbor made `_undef` (a typed one whose
                // prelude or block its type does not allow) is unnamed.
                let name = match kind {
                    AtKind::Undef => Vec::new(),
                    _ => {
                        let mut n = falloc::try_to_vec(c.name(name)).or_oom()?;
                        n.make_ascii_lowercase();
                        n
                    }
                };
                Some(Rule::At {
                    name,
                    prelude: slice_trim(c.css, prelude_begin, prelude_end)?,
                    rules: rules(c, block, inner)?,
                    declarations: block_declarations(c, block)?,
                })
            }
            // A block's declarations (an at-rule's are read above, a style
            // rule's from its own field) and anything else: not surfaced.
            _ => None,
        };
        if let Some(e) = entry {
            out.falloc_push(e).or_oom()?;
        }
    }
    Ok(out)
}

/// Every declaration of a block's declaration lists, in order.
fn block_declarations(c: &Conv, block: Option<ItemId>) -> Result<Vec<Decl>, Fail> {
    let mut out = Vec::new();
    for (id, item) in c.sheet.rules.children(block) {
        if let Item::DeclarationList { .. } = item {
            for d in declarations(c, Some(id))? {
                out.falloc_push(d).or_oom()?;
            }
        }
    }
    Ok(out)
}

/// Parse a verified UTF-8 stylesheet into owned Rust data.
pub fn parse(css: &[u8]) -> Result<Vec<Rule>, Fail> {
    let sheet = crate::selectors::parse_stylesheet(css).map_err(|st| match st {
        Status::ErrorMemoryAllocation => Fail::Oom,
        _ => Fail::Parse,
    })?;
    /* Levels 0..=MAX_DEPTH, as `depth > MAX_DEPTH` refused before. */
    rules(
        &Conv { css, sheet: &sheet },
        Some(sheet.root),
        Depth::top(MAX_DEPTH + 1),
    )
}
