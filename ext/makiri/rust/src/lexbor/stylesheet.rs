//! Lexbor's CSS stylesheet parser, walked into owned Rust values - phase one of
//! `Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array`. Phase two, turning
//! them into Ruby, is `glue::stylesheet`; this module knows nothing of Ruby.
//!
//! A deliberately thin binding, returning plain Ruby primitives so the
//! abstraction seam lives in the caller (dommy's `internal/css/parser.rb`), not
//! here. The shape is unchanged from the C:
//!
//! ```text
//! [ { type: :style,
//!     selectors:    [ { text: "div.a", specificity: [a, b, c] }, ... ],
//!     declarations: [ { name: "display", value: "flex", important: false }, ... ] },
//!   { type: :bad_style, selector_text: "p::before", declarations: [ ... ] },
//!   { type: :at_rule, name: "media", prelude: "(min-width: 600px)", rules: [ ... ] } ]
//! ```
//!
//! Error recovery follows css-syntax-3: a malformed declaration is dropped, an
//! unknown at-rule skipped, a selector list Lexbor rejects surfaces as
//! `:bad_style` with its raw prelude for the caller to re-validate - as does
//! one Lexbor accepted but `selector_text` cannot write back faithfully. A
//! broken stylesheet never raises; only a hard parser failure does.
//!
//! # Two phases, and why
//!
//! The C wrapped the whole conversion in `rb_ensure`, because it built Ruby
//! objects while the Lexbor parser and stylesheet were alive and a Ruby-level
//! raise (a `NoMemoryError` out of `rb_hash_new`, say) would `longjmp` past the
//! frees. Rust has the same hazard and a worse version of it: `longjmp` does not
//! run `Drop`, so an RAII guard would be no guard at all.
//!
//! So this does not build Ruby objects with Lexbor alive. Phase one walks the
//! parsed stylesheet into owned Rust values and drops every Lexbor resource;
//! phase two turns those into Ruby. Nothing that can raise runs while anything
//! needs freeing, which removes the problem rather than guarding it. Stylesheet
//! parsing is rare by design - once per `<style>`, not a hot path - so the
//! intermediate is not worth optimising away.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::falloc::{self, OomOption, OomResult, VecPush};
use crate::lexbor::abi as lxb;
use crate::lexbor::abi::consts as k;

use crate::lexbor::abi::{lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init};
use crate::lexbor::chunks::{chunk_cb, Chunks};
use crate::lexbor::css_engine::{lexbor_str, Owned};
use crate::lexbor::css_parser::list_from_raw;
use crate::lexbor::selector_text;

/// Bound on at-rule nesting: fail closed rather than recurse without limit on a
/// pathologically nested stylesheet.
pub const MAX_DEPTH: u32 = 64;

/* ------------------------------------------------------------------ *
 * phase one: Lexbor -> owned Rust                                    *
 * ------------------------------------------------------------------ */

pub struct Decl {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    pub important: bool,
}

pub struct Selector {
    pub text: Vec<u8>,
    /// `[a, b, c]` per Selectors L4 §17. The packed `!important` and
    /// style-attribute flags are never set on a parsed stylesheet rule, so they
    /// are dropped, exactly as the C did.
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
    },
}

/// Anything that stops phase one. `Oom` and `TooDeep` become `Makiri::Error`;
/// there is no syntax variant because Lexbor recovers from syntax errors and
/// this layer surfaces what it recovered.
pub enum Fail {
    Oom,
    TooDeep,
    /// The stylesheet parser could not be initialised.
    Init,
    /// Lexbor could not parse the stylesheet (normally an allocation failure).
    Parse,
    /// A Lexbor serializer returned non-OK. Named for what it is: the parser
    /// itself never fails this way - Lexbor recovers from CSS syntax errors and
    /// still returns OK, which is why there is no syntax variant.
    Serialize,
}

impl crate::falloc::Oom for Fail {
    #[inline]
    fn oom() -> Self {
        Fail::Oom
    }
}

/* ---- serialization ---- */

/// Drive one of Lexbor's `*_serialize` callbacks into an owned buffer.
///
/// `scratch` is reused across calls so a stylesheet's many small serializations
/// share one allocation, and is handed back grown.
unsafe fn serialize_with(
    scratch: &mut Vec<u8>,
    run: impl FnOnce(&mut Chunks<Vec<u8>>) -> u32,
) -> Result<Vec<u8>, Fail> {
    let mut s = Chunks::new(core::mem::take(scratch));
    s.sink.clear();
    let st = run(&mut s);
    /* Lexbor has returned: raise what the sink caught, giving the scratch
     * buffer back first so the caller's allocation is not lost. */
    *scratch = core::mem::take(&mut s.sink);
    s.panic.resume();
    if s.refused {
        return Err(Fail::Oom);
    }
    if st != 0 {
        return Err(Fail::Serialize);
    }
    falloc::try_to_vec(scratch).or_oom()
}

/* ---- specificity ---- */

/// `LXB_CSS_SELECTOR_SPECIFICITY_MASK` is `((1u32 << 23) - 1) << 9`, so each
/// component is the low 9 bits after its shift: a at 18, b at 9, c at 0.
const SP_COMPONENT: u32 = 0x1FF;

fn specificity(sp: u32) -> [u32; 3] {
    [
        (sp >> 18) & SP_COMPONENT,
        (sp >> 9) & SP_COMPONENT,
        sp & SP_COMPONENT,
    ]
}

/* ---- the walk ---- */

struct Conv<'a> {
    /// The original input, borrowed. Preludes - and a value a rewrite
    /// touched - are taken from it by Lexbor's offsets (`slice_trim`).
    css: &'a [u8],
    /// What Lexbor was actually given, when [`contains_guard`] rewrote a name.
    /// `None` when the two are the same, which is the usual case.
    ///
    /// [`contains_guard`]: crate::lexbor::contains_guard
    parsed: Option<&'a [u8]>,
    scratch: Vec<u8>,
}

unsafe fn declarations(
    c: &mut Conv,
    list: *mut lxb::lxb_css_rule_declaration_list_t,
) -> Result<Vec<Decl>, Fail> {
    let mut out = Vec::new();
    if list.is_null() {
        return Ok(out);
    }
    let mut r = (*list).first;
    while !r.is_null() {
        if (*r).type_ as usize == k::CSS_RULE_DECLARATION {
            // The downcast is Lexbor's own macro: every rule type begins with
            // the shared lxb_css_rule_t header, so this is a pointer cast.
            let decl = r as *mut lxb::lxb_css_rule_declaration_t;
            let style = (*decl).u.user;
            let ty = (*decl).type_;

            let name = serialize_with(&mut c.scratch, |s| {
                lxb::lxb_css_property_serialize_name(style, ty, Some(chunk_cb::<Vec<u8>>), s.ctx())
            })?;
            let value = serialize_with(&mut c.scratch, |s| {
                lxb::lxb_css_property_serialize(style, ty, Some(chunk_cb::<Vec<u8>>), s.ctx())
            })?;
            /* Serialized from what Lexbor parsed - the rewritten buffer - so a
             * rewritten name in a value (`--x: :lexbor-contains(1 2)`) came back
             * as `:zzzzzzzzzzzzzzz(1 2)`. Such a value is taken from the
             * caller's own text instead, by the offsets Lexbor recorded. */
            let value =
                if c.parsed.is_some() && crate::lexbor::contains_guard::may_hold_rewrite(&value) {
                    let off = (*decl).offset;
                    slice_trim(c.css, off.value_begin, off.value_end)?
                } else {
                    value
                };

            if out
                .falloc_push(Decl {
                    name,
                    value,
                    important: (*decl).important,
                })
                .is_err()
            {
                return Err(Fail::Oom);
            }
        }
        r = (*r).next;
    }
    Ok(out)
}

/// Each comma alternative of a style rule's selector list, as text
/// (`lexbor::selector_text`) with its specificity - or None when one of them
/// cannot be written back as it was written, for the caller to report the
/// rule as `:bad_style` rather than hand out a selector that means something
/// else.
///
/// # Safety
/// `sel` is null or the list of a style rule in a stylesheet that outlives
/// the call.
unsafe fn selectors(sel: *mut lxb::lxb_css_selector_list_t) -> Result<Option<Vec<Selector>>, Fail> {
    let mut out = Vec::new();
    // SAFETY: forwarded; the stylesheet is not touched while the view lives.
    for list in unsafe { list_from_raw(sel) } {
        let mut text = Vec::new();
        match selector_text::write(list, &mut text) {
            Ok(()) => {}
            Err(selector_text::Fail::Lossy) => return Ok(None),
            Err(selector_text::Fail::Oom) => return Err(Fail::Oom),
        }
        out.falloc_push(Selector {
            text,
            specificity: specificity(list.specificity()),
        })
        .or_oom()?;
    }
    Ok(Some(out))
}

/// Slice `[begin, end)` out of the original input, trimmed (`trim`) - the
/// one copy. Empty when the offsets are unusable - fail closed, never a wrong
/// slice.
///
/// Always the ORIGINAL input, never Lexbor's copy of the range: after a
/// `contains_guard` rewrite that copy is not what the caller typed, and
/// otherwise it is these very bytes (Lexbor copies `[begin, end)` of what it
/// read, and the rewrite keeps every byte where it was). Lexbor takes the
/// offsets from the tokens it read, so a range that does not fit is not
/// expected; were one to arise, the empty text is a value the caller already
/// has to refuse, unlike a range of someone else's text.
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
/// a prelude Lexbor rejected would come back as a different, valid selector.
/// (Trimming the space that ends a hex escape, `\31 `, leaves the escape at
/// the end, where it means the same.)
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

/// The at-rule keyword, without the `@`.
///
/// Typed at-rules carry no name field, so the name comes from the type; a
/// custom at-rule (any keyword Lexbor has no dedicated parser for - @supports,
/// @layer, @keyframes) keeps the verbatim ident. The on-rule `name_begin`
/// offset is NOT used: it is not reset between sibling rules.
unsafe fn at_name(at: *mut lxb::lxb_css_rule_at_t) -> Result<Vec<u8>, Fail> {
    let lit = |s: &[u8]| falloc::try_to_vec(s).or_oom();
    match (*at).type_ {
        k::AT_RULE_MEDIA => lit(b"media"),
        k::AT_RULE_FONT_FACE => lit(b"font-face"),
        k::AT_RULE_NAMESPACE => lit(b"namespace"),
        k::AT_RULE_CUSTOM => match (*at).u.custom.as_ref().and_then(|cu| lexbor_str(&cu.name)) {
            Some(name) => lit(name),
            None => Ok(Vec::new()),
        },
        // __UNDEF (malformed) and anything else: unnamed.
        _ => Ok(Vec::new()),
    }
}

/// The nested block of any at-rule that has one, or null for the statement
/// at-rules (@import, @namespace, @charset).
unsafe fn at_block(at: *mut lxb::lxb_css_rule_at_t) -> *mut lxb::lxb_css_rule_list_t {
    let null = core::ptr::null_mut;
    match (*at).type_ {
        k::AT_RULE_MEDIA => (*at).u.media.as_ref().map_or(null(), |m| m.block),
        k::AT_RULE_FONT_FACE => (*at).u.font_face.as_ref().map_or(null(), |f| f.block),
        k::AT_RULE_CUSTOM => (*at).u.custom.as_ref().map_or(null(), |cu| cu.block),
        k::AT_RULE_UNDEF => (*at).u.undef.as_ref().map_or(null(), |u| u.block),
        _ => null(),
    }
}

unsafe fn rules(
    c: &mut Conv,
    first: *mut lxb::lxb_css_rule_t,
    depth: u32,
) -> Result<Vec<Rule>, Fail> {
    if depth > MAX_DEPTH {
        return Err(Fail::TooDeep);
    }
    let mut out = Vec::new();
    let mut r = first;
    while !r.is_null() {
        let entry = match (*r).type_ as usize {
            k::CSS_RULE_STYLE => {
                let st = r as *mut lxb::lxb_css_rule_style_t;
                let declarations = declarations(c, (*st).declarations)?;
                Some(match selectors((*st).selector)? {
                    Some(selectors) => Rule::Style {
                        selectors,
                        declarations,
                    },
                    // Not writable as text: the caller re-validates the
                    // prelude as written, as for a selector Lexbor rejected.
                    None => Rule::BadStyle {
                        selector_text: slice_trim(c.css, (*st).prelude_begin, (*st).prelude_end)?,
                        declarations,
                    },
                })
            }
            k::CSS_RULE_AT_RULE => {
                let at = r as *mut lxb::lxb_css_rule_at_t;
                let block_first = at_block(at)
                    .as_ref()
                    .map_or(core::ptr::null_mut(), |b| b.first);
                let name = at_name(at)?;
                let prelude = slice_trim(c.css, (*at).prelude_begin, (*at).prelude_end)?;
                Some(Rule::At {
                    name,
                    prelude,
                    rules: rules(c, block_first, depth + 1)?,
                })
            }
            k::CSS_RULE_BAD_STYLE => {
                // A selector Lexbor rejected - pseudo-elements most notably,
                // since it is stricter/older than Selectors L4. Surface the raw
                // prelude so the caller can re-validate with its own parser
                // rather than lose the rule.
                let bad = r as *mut lxb::lxb_css_rule_bad_style_t;
                // As written and trimmed, like the prelude of a rule
                // `selectors` refuses, so a `:bad_style` reads the same
                // whichever refused it.
                Some(Rule::BadStyle {
                    selector_text: slice_trim(c.css, (*bad).prelude_begin, (*bad).prelude_end)?,
                    declarations: declarations(c, (*bad).declarations)?,
                })
            }
            // Anything else error recovery dropped: do not surface it.
            _ => None,
        };
        if let Some(e) = entry {
            out.falloc_push(e).or_oom()?;
        }
        r = (*r).next;
    }
    Ok(out)
}

/* ------------------------------------------------------------------ *
 * entry point                                                        *
 * ------------------------------------------------------------------ */

/// A stylesheet Lexbor parsed from what [`contains_guard`] let through, with
/// the parser that built it. Fields drop in declaration order: the parser
/// before the stylesheet, as the parse left them.
///
/// [`contains_guard`]: crate::lexbor::contains_guard
struct GuardedSheet {
    /// Held only to be dropped, first: nothing reads it after the parse.
    _parser: Owned<lxb::lxb_css_parser_t>,
    sheet: Owned<lxb::lxb_css_stylesheet_t>,
    /// The guard's rewrite - what Lexbor parsed - when it made one; `None`
    /// when Lexbor parsed the caller's text as it is, the usual case. A
    /// rewrite keeps byte length, so every span Lexbor reports still lands on
    /// the caller's text.
    parsed: Option<Vec<u8>>,
}

/// Parse `css` with Lexbor's stylesheet parser - the one call of it in the
/// crate (`rake unsafe:boundaries` pins it here), so no stylesheet reaches the
/// parser without going through `contains_guard` first, as no selector reaches
/// its parser without `css_engine::SelectorParser::parse`.
fn guarded_stylesheet(css: &[u8]) -> Result<GuardedSheet, Fail> {
    let parsed = crate::lexbor::contains_guard::neutralized(css).map_err(|_| Fail::Oom)?;
    // SAFETY: every pointer is created by the Lexbor calls below and owned by
    // an `Owned` from then on; the text is a live slice the parser only reads.
    unsafe {
        let sheet = Owned::new(
            lxb::lxb_css_stylesheet_create(core::ptr::null_mut()),
            lxb::lxb_css_stylesheet_destroy,
        )
        .ok_or(Fail::Init)?;
        let parser =
            Owned::new(lxb_css_parser_create(), lxb_css_parser_destroy).ok_or(Fail::Init)?;
        if lxb_css_parser_init(parser.as_ptr(), core::ptr::null_mut()) != k::STATUS_OK {
            return Err(Fail::Init);
        }
        let source = parsed.as_deref().unwrap_or(css);
        if lxb::lxb_css_stylesheet_parse(
            sheet.as_ptr(),
            parser.as_ptr(),
            source.as_ptr(),
            source.len(),
        ) != k::STATUS_OK
        {
            return Err(Fail::Parse);
        }
        Ok(GuardedSheet {
            _parser: parser,
            sheet,
            parsed,
        })
    }
}

/// Parse a verified UTF-8 stylesheet into owned Rust data.
///
/// This is the safe boundary consumed by the Ruby glue: all Lexbor-owned
/// pointers and callback state have been dropped before it returns.
///
/// The parser and the stylesheet are owned by [`GuardedSheet`] for the length
/// of phase one, so every path out frees exactly what was created. That `Drop`
/// is sound because nothing between construction and drop can `longjmp`: phase
/// one calls Lexbor and the allocator, never Ruby.
pub fn parse(css: &[u8]) -> Result<Vec<Rule>, Fail> {
    let sheet = guarded_stylesheet(css)?;
    // SAFETY: the stylesheet `sheet` owns, live until it drops at the end of
    // this function; nothing in here runs Ruby.
    unsafe {
        let root = (*sheet.sheet.as_ptr()).root;
        if root.is_null() {
            return Ok(Vec::new());
        }
        let mut conv = Conv {
            css,
            parsed: sheet.parsed.as_deref(),
            scratch: Vec::new(),
        };
        let first = (*(root as *mut lxb::lxb_css_rule_list_t)).first;
        rules(&mut conv, first, 0)
    }
}
