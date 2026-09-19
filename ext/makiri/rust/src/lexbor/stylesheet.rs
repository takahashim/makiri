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
//! `:bad_style` with its raw prelude for the caller to re-validate. A broken
//! stylesheet never raises; only a hard parser failure does.
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

use core::ffi::c_void;

use crate::falloc::{self, VecPush};
use crate::lexbor_abi as lxb;
use crate::lexbor_abi::consts as k;

use crate::lexbor_abi::{
    lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init, CssParser,
};

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

/* ---- serialization ---- */

/// Collects Lexbor's serializer chunks. `oom` latches, and the callback then
/// stops the serializer by returning a non-OK status.
struct Ser {
    buf: Vec<u8>,
    oom: bool,
    /// A panic, latched like `oom`: this is called from C, and unwinding into
    /// Lexbor aborts. `serialize_with` raises it after the call returns.
    panic: crate::caught::PanicLatch,
}

/// Lexbor's chunk sink. Must not panic INTO C: a panic is latched and raised by
/// `serialize_with` once Lexbor has returned, as an allocation failure is.
unsafe extern "C" fn ser_cb(data: *const u8, len: usize, ctx: *mut c_void) -> u32 {
    let s = &mut *(ctx as *mut Ser);
    let (buf, oom) = (&mut s.buf, &mut s.oom);
    /* Any non-OK status stops the serializer. */
    s.panic.guard(k::STATUS_ERROR, || {
        if len != 0 && !data.is_null() {
            // SAFETY: Lexbor hands `len` readable bytes at `data`.
            let bytes = unsafe { core::slice::from_raw_parts(data, len) };
            if buf.mkr_extend(bytes).is_err() {
                *oom = true;
                return k::STATUS_ERROR;
            }
        }
        k::STATUS_OK
    })
}

/// Drive one of Lexbor's `*_serialize` callbacks into an owned buffer.
///
/// `scratch` is reused across calls so a stylesheet's many small serializations
/// share one allocation, and is handed back grown.
unsafe fn serialize_with(
    scratch: &mut Vec<u8>,
    run: impl FnOnce(&mut Ser) -> u32,
) -> Result<Vec<u8>, Fail> {
    let mut s = Ser {
        buf: core::mem::take(scratch),
        oom: false,
        panic: crate::caught::PanicLatch::new(),
    };
    s.buf.clear();
    let st = run(&mut s);
    /* Lexbor has returned: raise what the sink caught, giving the scratch
     * buffer back first so the caller's allocation is not lost. */
    if s.panic.caught() {
        *scratch = core::mem::take(&mut s.buf);
        s.panic.resume();
    }
    if s.oom {
        *scratch = s.buf;
        return Err(Fail::Oom);
    }
    if st != 0 {
        *scratch = s.buf;
        return Err(Fail::Serialize);
    }
    let out = match falloc::try_to_vec(&s.buf) {
        Some(v) => v,
        None => {
            *scratch = s.buf;
            return Err(Fail::Oom);
        }
    };
    *scratch = s.buf;
    Ok(out)
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
    /// The original input, borrowed. At-rule preludes are byte ranges into it.
    css: &'a [u8],
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
                lxb::lxb_css_property_serialize_name(
                    style,
                    ty,
                    Some(ser_cb),
                    s as *mut Ser as *mut c_void,
                )
            })?;
            let value = serialize_with(&mut c.scratch, |s| {
                lxb::lxb_css_property_serialize(
                    style,
                    ty,
                    Some(ser_cb),
                    s as *mut Ser as *mut c_void,
                )
            })?;

            if out
                .mkr_push(Decl {
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

unsafe fn selectors(
    c: &mut Conv,
    sel: *mut lxb::lxb_css_selector_list_t,
) -> Result<Vec<Selector>, Fail> {
    let mut out = Vec::new();
    let mut l = sel;
    while !l.is_null() {
        // One comma branch only: serialize THIS list's chain without following
        // list->next, which would re-emit the whole comma list.
        let first = (*l).first;
        let text = serialize_with(&mut c.scratch, |s| {
            lxb::lxb_css_selector_serialize_chain(first, Some(ser_cb), s as *mut Ser as *mut c_void)
        })?;
        let sp = specificity((*l).specificity);
        if out
            .mkr_push(Selector {
                text,
                specificity: sp,
            })
            .is_err()
        {
            return Err(Fail::Oom);
        }
        l = (*l).next;
    }
    Ok(out)
}

/// Slice `[begin, end)` out of the original input, trimming ASCII whitespace.
/// Empty when the offsets are unusable - fail closed, never a wrong slice.
fn slice_trim(css: &[u8], begin: usize, end: usize) -> Result<Vec<u8>, Fail> {
    if begin > end || end > css.len() {
        return Ok(Vec::new());
    }
    let mut s = &css[begin..end];
    while let Some((&b, rest)) = s.split_first() {
        if b > b' ' {
            break;
        }
        s = rest;
    }
    while let Some((&b, rest)) = s.split_last() {
        if b > b' ' {
            break;
        }
        s = rest;
    }
    falloc::try_to_vec(s).ok_or(Fail::Oom)
}

/// The at-rule keyword, without the `@`.
///
/// Typed at-rules carry no name field, so the name comes from the type; a
/// custom at-rule (any keyword Lexbor has no dedicated parser for - @supports,
/// @layer, @keyframes) keeps the verbatim ident. The on-rule `name_begin`
/// offset is NOT used: it is not reset between sibling rules.
unsafe fn at_name(at: *mut lxb::lxb_css_rule_at_t) -> Result<Vec<u8>, Fail> {
    let lit = |s: &[u8]| falloc::try_to_vec(s).ok_or(Fail::Oom);
    match (*at).type_ {
        k::AT_RULE_MEDIA => lit(b"media"),
        k::AT_RULE_FONT_FACE => lit(b"font-face"),
        k::AT_RULE_NAMESPACE => lit(b"namespace"),
        k::AT_RULE_CUSTOM => {
            let cu = (*at).u.custom;
            if !cu.is_null() && !(*cu).name.data.is_null() {
                let bytes =
                    core::slice::from_raw_parts((*cu).name.data as *const u8, (*cu).name.length);
                lit(bytes)
            } else {
                Ok(Vec::new())
            }
        }
        // __UNDEF (malformed) and anything else: unnamed.
        _ => Ok(Vec::new()),
    }
}

/// The nested block of any at-rule that has one, or null for the statement
/// at-rules (@import, @namespace, @charset).
unsafe fn at_block(at: *mut lxb::lxb_css_rule_at_t) -> *mut lxb::lxb_css_rule_list_t {
    match (*at).type_ {
        k::AT_RULE_MEDIA => {
            let m = (*at).u.media;
            if m.is_null() {
                core::ptr::null_mut()
            } else {
                (*m).block
            }
        }
        k::AT_RULE_FONT_FACE => {
            let f = (*at).u.font_face;
            if f.is_null() {
                core::ptr::null_mut()
            } else {
                (*f).block
            }
        }
        k::AT_RULE_CUSTOM => {
            let cu = (*at).u.custom;
            if cu.is_null() {
                core::ptr::null_mut()
            } else {
                (*cu).block
            }
        }
        k::AT_RULE_UNDEF => {
            let u = (*at).u.undef;
            if u.is_null() {
                core::ptr::null_mut()
            } else {
                (*u).block
            }
        }
        _ => core::ptr::null_mut(),
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
                Some(Rule::Style {
                    selectors: selectors(c, (*st).selector)?,
                    declarations: declarations(c, (*st).declarations)?,
                })
            }
            k::CSS_RULE_AT_RULE => {
                let at = r as *mut lxb::lxb_css_rule_at_t;
                let block = at_block(at);
                let block_first = if block.is_null() {
                    core::ptr::null_mut()
                } else {
                    (*block).first
                };
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
                let text = if (*bad).selectors.data.is_null() {
                    Vec::new()
                } else {
                    let b = core::slice::from_raw_parts(
                        (*bad).selectors.data as *const u8,
                        (*bad).selectors.length,
                    );
                    falloc::try_to_vec(b).ok_or(Fail::Oom)?
                };
                Some(Rule::BadStyle {
                    selector_text: text,
                    declarations: declarations(c, (*bad).declarations)?,
                })
            }
            // Anything else error recovery dropped: do not surface it.
            _ => None,
        };
        if let Some(e) = entry {
            if out.mkr_push(e).is_err() {
                return Err(Fail::Oom);
            }
        }
        r = (*r).next;
    }
    Ok(out)
}

/* ------------------------------------------------------------------ *
 * entry point                                                        *
 * ------------------------------------------------------------------ */

/// Owns the parser and stylesheet for the length of phase one.
///
/// A `Drop` guard is sound HERE, unlike in the C's arrangement, precisely
/// because nothing between construction and drop can `longjmp`: phase one calls
/// Lexbor and the allocator, never Ruby.
struct Engine {
    parser: *mut CssParser,
    sst: *mut lxb::lxb_css_stylesheet_t,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: this type owns both objects - `parse` is the only constructor
        // and never hands either pointer out - so neither can be destroyed
        // elsewhere. A half-built engine leaves the failed half null, which the
        // guards below skip, so an `Err` return from `parse` frees exactly what
        // it managed to create.
        unsafe {
            if !self.sst.is_null() {
                lxb::lxb_css_stylesheet_destroy(self.sst, true);
            }
            if !self.parser.is_null() {
                lxb_css_parser_destroy(self.parser, true);
            }
        }
    }
}

/// Parse a verified UTF-8 stylesheet into owned Rust data.
///
/// This is the safe boundary consumed by the Ruby glue: all Lexbor-owned
/// pointers and callback state have been dropped before it returns.
pub fn parse(css: &[u8]) -> Result<Vec<Rule>, Fail> {
    // SAFETY: one contract for the whole body. Every pointer here is created by
    // the Lexbor calls below, null-checked before use, and owned by `eng`,
    // whose `Drop` frees it on every path out. `css` is a Rust slice, which the
    // parser only reads, and nothing in here runs Ruby.
    unsafe {
        let eng = Engine {
            parser: lxb_css_parser_create(),
            sst: lxb::lxb_css_stylesheet_create(core::ptr::null_mut()),
        };
        if eng.parser.is_null()
            || eng.sst.is_null()
            || lxb_css_parser_init(eng.parser, core::ptr::null_mut()) != 0
        {
            return Err(Fail::Init);
        }
        if lxb::lxb_css_stylesheet_parse(
            eng.sst,
            eng.parser as *mut lxb::lxb_css_parser_t,
            css.as_ptr(),
            css.len(),
        ) != 0
        {
            return Err(Fail::Parse);
        }
        let root = (*eng.sst).root;
        if root.is_null() {
            return Ok(Vec::new());
        }
        let mut conv = Conv {
            css,
            scratch: Vec::new(),
        };
        let first = (*(root as *mut lxb::lxb_css_rule_list_t)).first;
        rules(&mut conv, first, 0)
    }
}
