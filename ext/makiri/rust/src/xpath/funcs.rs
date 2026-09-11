//! The built-in XPath 1.0 function library (mkr_xpath_funcs_body.h), plus the
//! two Nokogiri-compatible builtins and the CSS lowering's internal hooks.
//!
//! The C dispatches through a table of function pointers; here it is a match on
//! the name, which the compiler turns into the same thing per backend. The
//! host-policy branches the C spells `#ifdef MKR_HOST_XML` are `D::IS_XML`.

use super::abi::*;
use super::dom::*;
use super::value::*;
use crate::err_setf;
use core::ffi::{c_char, c_void};
use core::ptr;

/// Namespace URIs registered from Nokogiri's XPath context, so prefixed names
/// like "nokogiri-builtin:css-class" resolve.
pub const NS_NOKOGIRI_BUILTIN_URI: &[u8] =
    b"https://www.nokogiri.org/default_ns/ruby/builtins";

/// Names the CSS lowering emits for an untyped `:*-of-type`, where the "type" is
/// the element's own expanded name - a self-reference XPath 1.0 cannot express
/// (there is no current()). The leading \x01 cannot come out of the lexer, so
/// these are unreachable from a user expression. XML host only.
pub const FN_OF_TYPE_POS: &[u8] = b"\x01of-type-pos";
pub const FN_OF_TYPE_POS_LAST: &[u8] = b"\x01of-type-pos-last";

/// The dynamic context of XPath 1.0: the context node with its 1-based position
/// and the context size. These three always travel together.
#[derive(Clone, Copy)]
pub struct Focus<D: Dom> {
    pub node: D::Node,
    pub pos: usize,
    pub size: usize,
}

/* ---------- small helpers ---------- */

unsafe fn arity(got: usize, min: usize, max: usize, err: *mut Error, name: &str) -> bool {
    if got < min || got > max {
        if min == max {
            err_setf!(err, XP_ERR_RUNTIME, "{}(): expected {} argument(s), got {}", name, min, got);
        } else {
            err_setf!(
                err,
                XP_ERR_RUNTIME,
                "{}(): expected {}-{} argument(s), got {}",
                name,
                min,
                max,
                got
            );
        }
        return false;
    }
    true
}

/// The shared "argument must be a node-set" check.
unsafe fn require_nodeset(arg: *const Val, fname: &str, err: *mut Error) -> Option<*const NodeSet> {
    if (*arg).type_ != T_NODESET {
        err_setf!(err, XP_ERR_TYPE, "{}(): argument must be a node-set", fname);
        return None;
    }
    Some(&raw const (*arg).u.nodeset)
}

/// An owned C string holding `s`, or None on OOM. The result is freed by C, so
/// it comes from the C allocator.
unsafe fn c_string(s: &[u8], err: *mut Error, what: &str) -> Option<OwnedText> {
    let p = mkr_str_alloc(s.len());
    if p.is_null() {
        err_setf!(err, XP_ERR_OOM, "out of memory in {}()", what);
        return None;
    }
    if !s.is_empty() {
        ptr::copy_nonoverlapping(s.as_ptr(), p as *mut u8, s.len());
    }
    *p.add(s.len()) = 0;
    Some(OwnedText { ptr: p, len: s.len() })
}

unsafe fn set_string(out: *mut Val, s: &[u8], err: *mut Error, what: &str) -> bool {
    match c_string(s, err, what) {
        Some(t) => {
            mkr_val_set_owned_text(out, t);
            true
        }
        None => false,
    }
}

/// An owned text guard: clears the C allocation when it goes out of scope, so
/// the many early returns below need no per-site cleanup.
struct Text(OwnedText);

impl Text {
    fn new() -> Text {
        Text(OwnedText { ptr: ptr::null_mut(), len: 0 })
    }
    fn as_slice(&self) -> &[u8] {
        unsafe { owned_bytes(self.0) }
    }
    fn as_mut(&mut self) -> *mut OwnedText {
        &mut self.0
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        unsafe { mkr_owned_text_clear(&mut self.0) }
    }
}

unsafe fn to_text<D: Dom>(v: *const Val, ctx: *mut Context, err: *mut Error) -> Option<Text> {
    let mut t = Text::new();
    if val_to_owned_text_or_fail::<D>(v, mkr_ctx_limits(ctx), err, t.as_mut()) {
        Some(t)
    } else {
        None
    }
}

unsafe fn to_number<D: Dom>(v: *const Val, ctx: *mut Context, err: *mut Error) -> Option<f64> {
    let mut d = 0.0;
    if val_to_number_or_fail::<D>(v, mkr_ctx_limits(ctx), err, &mut d) {
        Some(d)
    } else {
        None
    }
}

/// The string-value of `args[0]`, or of the context node when there is none -
/// the idiom string() / string-length() / normalize-space() share.
unsafe fn arg_or_self_text<D: Dom>(
    focus: &Focus<D>,
    args: &[Val],
    ctx: *mut Context,
    err: *mut Error,
) -> Option<Text> {
    if args.is_empty() {
        let mut t = Text::new();
        if node_to_owned_text::<D>(focus.node, mkr_ctx_limits(ctx), err, t.as_mut()) {
            Some(t)
        } else {
            None
        }
    } else {
        to_text::<D>(&args[0], ctx, err)
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/* ---------- id() ---------- */

/// Walk the tree for an element whose `id` attribute is `id`.
///
/// Every visited node is charged to the op budget: without it, id() over a large
/// node-set - a token per node, a tree walk per token - drives quadratic work at
/// no cost. Returns Err(()) on an overrun, with `*err` set.
unsafe fn find_by_id<D: Dom>(
    root: D::Node,
    id: &[u8],
    limits: *mut Limits,
    err: *mut Error,
) -> Result<D::Node, ()> {
    if D::is_null(root) || id.is_empty() {
        return Ok(D::null());
    }
    let mut n = root;
    while !D::is_null(n) {
        if mkr_limit_eval_op(limits, err) != 0 {
            return Err(());
        }
        if D::node_type(n) == NTYPE_ELEMENT && D::get_attribute(n, b"id") == Some(id) {
            return Ok(n);
        }
        if !D::is_null(D::first_child(n)) {
            n = D::first_child(n);
        } else {
            while !D::is_null(n) && n != root && D::is_null(D::next(n)) {
                n = D::parent(n);
            }
            if D::is_null(n) || n == root {
                break;
            }
            n = D::next(n);
        }
    }
    Ok(D::null())
}

/// Look up every whitespace-separated token of `s` and push each hit.
///
/// Duplicates go in unconditionally: the caller dedups the whole result with one
/// sort plus an adjacent pass, which beats a contains() check per insert.
unsafe fn id_collect<D: Dom>(
    s: &[u8],
    root: D::Node,
    out: *mut NodeSet,
    ctx: *mut Context,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    for tok in s.split(|&b| super::lex::is_ws(b)).filter(|t| !t.is_empty()) {
        let hit = match find_by_id::<D>(root, tok, limits, err) {
            Ok(h) => h,
            Err(()) => return false,
        };
        if !D::is_null(hit) && mkr_nodeset_push(out, D::to_void(hit), limits, err) != 0 {
            return false;
        }
    }
    true
}

unsafe fn fn_id<D: Dom>(
    ctx: *mut Context,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    (*out).type_ = T_NODESET;
    mkr_nodeset_init(&raw mut (*out).u.nodeset);

    if D::IS_XML {
        /* Host policy: in XML an ID is an attribute DECLARED ID-typed by the
         * DTD, not any attribute named "id". DTDs are rejected at parse, so a
         * document read here carries no ID-typed attributes and id() is the
         * empty node-set. (xml:id is a separate, optional spec.) */
        return true;
    }
    let doc = mkr_ctx_document(ctx);
    if doc.is_null() {
        return true;
    }
    let root = D::from_void(doc);
    let ns_out = &raw mut (*out).u.nodeset;

    /* §4.1: a node-set argument treats each node's string-value as IDREFS;
     * anything else is converted to a string and split the same way. */
    let ok = if (*args.as_ptr()).type_ == T_NODESET {
        let set = &raw const (*args.as_ptr()).u.nodeset;
        (0..(*set).count).all(|i| {
            let mut t = Text::new();
            if !node_to_owned_text::<D>(nodeset_at::<D>(set, i), mkr_ctx_limits(ctx), err, t.as_mut())
            {
                return false;
            }
            id_collect::<D>(t.as_slice(), root, ns_out, ctx, err)
        })
    } else {
        match to_text::<D>(&args[0], ctx, err) {
            Some(t) => id_collect::<D>(t.as_slice(), root, ns_out, ctx, err),
            None => false,
        }
    };
    if !ok {
        mkr_nodeset_clear(ns_out);
        return false;
    }
    /* §4.1: the result is in document order with duplicates removed. */
    nodeset_unique_sorted::<D>(ctx, ns_out);
    true
}

/* ---------- the Nokogiri builtins ---------- */

/// css-class(haystack, needle): true iff `needle` is a whitespace-separated
/// token of `haystack`. Kept behaviour-identical to libxml2's builtin_css_class,
/// including the NULL ordering - a NULL haystack is a non-match even for an
/// empty needle.
fn ws_token_match(hay: Option<&[u8]>, val: Option<&[u8]>) -> bool {
    let (hay, val) = match (hay, val) {
        (Some(h), Some(v)) => (h, v),
        _ => return false,
    };
    if val.is_empty() {
        return true; /* libxml2 returns non-NULL for an empty val */
    }
    hay.split(|&b| super::lex::is_ws(b)).any(|t| t == val)
}

/* ---------- the CSS-lowered of-type hooks (XML only) ---------- */

/// Two elements are the same "type" iff they share an expanded name: local name
/// plus namespace URI.
unsafe fn same_type<D: Dom>(a: D::Node, b: D::Node, doc: D::Doc) -> bool {
    D::local_name(a) == D::local_name(b) && D::ns_uri(a, doc) == D::ns_uri(b, doc)
}

/// The 1-based position of `self` among its same-type element siblings: forward
/// counts the preceding siblings, otherwise the following ones (from the end).
unsafe fn of_type_pos<D: Dom>(node: D::Node, forward: bool, doc: D::Doc) -> f64 {
    if D::is_null(node) || D::node_type(node) != NTYPE_ELEMENT {
        return 0.0;
    }
    let step = |n: D::Node| if forward { D::prev(n) } else { D::next(n) };
    let mut pos = 1i64;
    let mut s = step(node);
    while !D::is_null(s) {
        if D::node_type(s) == NTYPE_ELEMENT && same_type::<D>(node, s, doc) {
            pos += 1;
        }
        s = step(s);
    }
    pos as f64
}

/* ---------- string functions ---------- */

unsafe fn fn_concat<D: Dom>(
    ctx: *mut Context,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    let mut parts: Vec<Text> = Vec::new();
    if parts.try_reserve(args.len()).is_err() {
        err_setf!(err, XP_ERR_OOM, "out of memory in concat()");
        return false;
    }
    let mut total = 0usize;
    for a in args {
        let t = match to_text::<D>(a, ctx, err) {
            Some(t) => t,
            None => return false,
        };
        total = match total.checked_add(t.0.len) {
            Some(n) => n,
            None => {
                err_setf!(err, XP_ERR_OOM, "concat() size overflow");
                return false;
            }
        };
        if mkr_limit_check_string_bytes(limits, total, err) != 0 {
            return false;
        }
        parts.push(t);
    }
    let buf = mkr_str_alloc(total);
    if buf.is_null() {
        err_setf!(err, XP_ERR_OOM, "out of memory in concat()");
        return false;
    }
    let mut off = 0usize;
    for p in &parts {
        let s = p.as_slice();
        ptr::copy_nonoverlapping(s.as_ptr(), (buf as *mut u8).add(off), s.len());
        off += s.len();
    }
    *buf.add(total) = 0;
    mkr_val_set_owned_text(out, OwnedText { ptr: buf, len: total });
    true
}

/// substring(s, start[, length]). Positions are 1-based character offsets that
/// round to nearest, and out-of-range positions clip silently.
unsafe fn fn_substring<D: Dom>(
    ctx: *mut Context,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let s = match to_text::<D>(&args[0], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let start_d = match to_number::<D>(&args[1], ctx, err) {
        Some(d) => d,
        None => return false,
    };
    let bytes = s.as_slice();
    let nchars = count_chars(bytes);
    let end_d = if args.len() == 3 {
        match to_number::<D>(&args[2], ctx, err) {
            Some(d) => start_d + d,
            None => return false,
        }
    } else {
        nchars as f64 + 1.0
    };

    if start_d.is_nan() || end_d.is_nan() {
        return set_string(out, b"", err, "substring");
    }
    /* Round, then clamp AS DOUBLES before any cast: start/end can be infinite
     * or beyond i64 (`substring(s, 1 div 0)`), where casting first would be
     * undefined in C and saturating here - either way not the spec's clip. */
    let imax = nchars as f64 + 1.0;
    let rstart = (start_d + 0.5).floor().clamp(1.0, imax);
    let rend = (end_d + 0.5).floor().clamp(1.0, imax);
    if rend <= rstart {
        return set_string(out, b"", err, "substring");
    }
    let from = advance_chars(bytes, (rstart as i64 - 1) as usize);
    let to = from + advance_chars(&bytes[from..], (rend as i64 - rstart as i64) as usize);
    set_string(out, &bytes[from..to], err, "substring")
}

/// The number of characters in valid UTF-8; a stray continuation byte counts as
/// its own character, matching the C primitive's byte-lead counting.
fn count_chars(s: &[u8]) -> usize {
    s.iter().filter(|&&b| (b & 0xC0) != 0x80).count()
}

/// The byte offset `n` characters into `s`, clipped to its length.
fn advance_chars(s: &[u8], n: usize) -> usize {
    let mut i = 0;
    let mut seen = 0;
    while i < s.len() && seen < n {
        i += 1;
        while i < s.len() && (s[i] & 0xC0) == 0x80 {
            i += 1;
        }
        seen += 1;
    }
    i
}

/// normalize-space: collapse runs of whitespace and trim the ends.
unsafe fn fn_normalize_space<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let s = match arg_or_self_text::<D>(focus, args, ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let src = s.as_slice();
    let buf = mkr_str_alloc(src.len());
    if buf.is_null() {
        err_setf!(err, XP_ERR_OOM, "out of memory in normalize-space()");
        return false;
    }
    let dst = core::slice::from_raw_parts_mut(buf as *mut u8, src.len() + 1);
    let mut w = 0usize;
    let mut in_space = true;
    for &c in src {
        if super::lex::is_ws(c) {
            if !in_space && w > 0 {
                dst[w] = b' ';
                w += 1;
            }
            in_space = true;
        } else {
            dst[w] = c;
            w += 1;
            in_space = false;
        }
    }
    if w > 0 && dst[w - 1] == b' ' {
        w -= 1;
    }
    dst[w] = 0;
    mkr_val_set_owned_text(out, OwnedText { ptr: buf, len: w });
    true
}

/// translate(s, from, to) works on CHARACTERS, not bytes: each code point of `s`
/// that appears in `from` becomes the code point at the same position in `to`,
/// or is dropped when `from` is longer.
///
/// The input is valid UTF-8 (the literal lexer validates, and DOM string-values
/// are valid), but a decode failure fails closed rather than truncating.
unsafe fn fn_translate<D: Dom>(
    ctx: *mut Context,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let limits = mkr_ctx_limits(ctx);
    let s = match to_text::<D>(&args[0], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let from = match to_text::<D>(&args[1], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let to = match to_text::<D>(&args[2], ctx, err) {
        Some(t) => t,
        None => return false,
    };

    let (sv, fv, tv) = match (
        core::str::from_utf8(s.as_slice()),
        core::str::from_utf8(from.as_slice()),
        core::str::from_utf8(to.as_slice()),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => {
            err_setf!(err, XP_ERR_RUNTIME, "invalid UTF-8 in translate() argument");
            return false;
        }
    };
    let from_cp: Vec<char> = fv.chars().collect();
    let to_cp: Vec<char> = tv.chars().collect();

    /* Capped: a multibyte replacement can push the result past the limit even
     * when the input is inside it ("a" -> an emoji), so the append fails closed
     * with LIMIT or OOM. */
    let mut buf = Buf::new((*limits).max_string_bytes);
    let mut enc = [0u8; 4];
    for c in sv.chars() {
        let emit: Option<&str> = match from_cp.iter().position(|&f| f == c) {
            None => Some(c.encode_utf8(&mut enc)),          /* not in `from`: keep it */
            Some(k) if k < to_cp.len() => Some(to_cp[k].encode_utf8(&mut enc)),
            Some(_) => None,                                 /* past `to`: drop it */
        };
        if let Some(e) = emit {
            let st = mkr_buf_append(&mut buf, e.as_ptr() as *const c_void, e.len());
            if st != ST_OK {
                buf.free();
                if st == ST_ERR_LIMIT {
                    err_setf!(
                        err,
                        XP_ERR_LIMIT,
                        "string size limit exceeded ({} bytes) in translate()",
                        (*limits).max_string_bytes
                    );
                } else {
                    err_setf!(err, XP_ERR_OOM, "out of memory in translate()");
                }
                return false;
            }
        }
    }
    let mut len = 0usize;
    let p = mkr_buf_steal(&mut buf, &mut len);
    if p.is_null() {
        err_setf!(err, XP_ERR_OOM, "out of memory in translate()");
        return false;
    }
    mkr_val_set_owned_text(out, OwnedText { ptr: p, len });
    true
}

/* ---------- name functions ---------- */

/// The first node of a node-set argument, or the context node when there is no
/// argument. A type error sets `*err`; an empty node-set yields a null handle,
/// which the callers render as "".
unsafe fn name_target<D: Dom>(
    args: &[Val],
    focus: &Focus<D>,
    err: *mut Error,
    fname: &str,
) -> Option<D::Node> {
    if args.is_empty() {
        return Some(focus.node);
    }
    let ns = require_nodeset(&args[0], fname, err)?;
    if (*ns).count == 0 {
        Some(D::null())
    } else {
        Some(nodeset_at::<D>(ns, 0))
    }
}

/// `n`'s local or qualified name as a string result; anything that is not an
/// element, attribute or PI yields "". A PI's name is its target either way (its
/// expanded-name is (null, target)). In HTML the qualified name equals the local
/// name, which also keeps the LXB_NS_HTML prefix out of the result.
unsafe fn name_emit<D: Dom>(
    n: D::Node,
    qualified: bool,
    out: *mut Val,
    err: *mut Error,
    fname: &str,
) -> bool {
    if D::is_null(n) {
        return set_string(out, b"", err, fname);
    }
    let name: &[u8] = match D::node_type(n) {
        NTYPE_ATTRIBUTE => {
            if qualified {
                D::attr_qualified_name(n)
            } else {
                D::attr_local_name(n)
            }
        }
        NTYPE_ELEMENT => {
            if qualified {
                D::qualified_name(n)
            } else {
                D::local_name(n)
            }
        }
        NTYPE_PI => D::pi_name(n),
        _ => b"",
    };
    set_string(out, name, err, fname)
}

/* ---------- lang() ---------- */

unsafe fn fn_lang<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let want = match to_text::<D>(&args[0], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let want = want.as_slice();
    *out = val_boolean(false);
    /* Walk the ancestors for the host's language attribute. Host policy: XPath
     * 1.0 lang() is xml:lang based; HTML uses `lang`, accepting xml:lang as a
     * fallback. */
    let mut p = focus.node;
    while !D::is_null(p) {
        if D::node_type(p) == NTYPE_ELEMENT {
            let v = if D::IS_XML {
                D::get_attribute(p, b"xml:lang")
            } else {
                D::get_attribute(p, b"lang").or_else(|| D::get_attribute(p, b"xml:lang"))
            };
            if let Some(v) = v {
                /* Case-insensitive compare of the prefix up to a '-'. */
                if v.len() >= want.len()
                    && v[..want.len()].eq_ignore_ascii_case(want)
                    && (v.len() == want.len() || v[want.len()] == b'-')
                {
                    (*out).u.boolean = 1;
                    break;
                }
            }
        }
        p = D::parent(p);
    }
    true
}

/* ---------- dispatch ---------- */

/// Is `(ns_uri, local)` a built-in? The evaluator asks before routing a call to
/// a Ruby handler.
pub fn is_builtin<D: Dom>(ns_uri: Option<&[u8]>, local: &[u8]) -> bool {
    match ns_uri {
        Some(uri) => {
            uri == NS_NOKOGIRI_BUILTIN_URI && matches!(local, b"css-class" | b"local-name-is")
        }
        None => matches!(
            local,
            b"last"
                | b"position"
                | b"count"
                | b"id"
                | b"local-name"
                | b"namespace-uri"
                | b"name"
                | b"string"
                | b"concat"
                | b"starts-with"
                | b"contains"
                | b"substring-before"
                | b"substring-after"
                | b"substring"
                | b"string-length"
                | b"normalize-space"
                | b"translate"
                | b"not"
                | b"true"
                | b"false"
                | b"boolean"
                | b"lang"
                | b"number"
                | b"sum"
                | b"floor"
                | b"ceiling"
                | b"round"
        ) || (D::IS_XML && (local == FN_OF_TYPE_POS || local == FN_OF_TYPE_POS_LAST)),
    }
}

/// Call the built-in named `(ns_uri, local)`. The caller has already confirmed
/// it exists with `is_builtin`; `out` starts zeroed and the caller clears it on
/// a false return.
///
/// # Safety
/// `ctx`, `args` and `out` must be valid, and the focus node live.
pub unsafe fn call<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    ns_uri: Option<&[u8]>,
    local: &[u8],
    args: &[Val],
    out: *mut Val,
    err: *mut Error,
) -> bool {
    let n = args.len();
    let limits = mkr_ctx_limits(ctx);

    if let Some(uri) = ns_uri {
        if uri != NS_NOKOGIRI_BUILTIN_URI {
            return false;
        }
        return match local {
            b"css-class" => {
                if !arity(n, 2, 2, err, "nokogiri-builtin:css-class") {
                    return false;
                }
                let hay = match to_text::<D>(&args[0], ctx, err) {
                    Some(t) => t,
                    None => return false,
                };
                let needle = match to_text::<D>(&args[1], ctx, err) {
                    Some(t) => t,
                    None => return false,
                };
                *out = val_boolean(ws_token_match(Some(hay.as_slice()), Some(needle.as_slice())));
                true
            }
            b"local-name-is" => {
                if !arity(n, 1, 1, err, "nokogiri-builtin:local-name-is") {
                    return false;
                }
                let want = match to_text::<D>(&args[0], ctx, err) {
                    Some(t) => t,
                    None => return false,
                };
                let hit = !D::is_null(focus.node) && D::qualified_name(focus.node) == want.as_slice();
                *out = val_boolean(hit);
                true
            }
            _ => false,
        };
    }

    match local {
        /* --- node-set --- */
        b"last" => arity(n, 0, 0, err, "last") && set_num(out, focus.size as f64),
        b"position" => arity(n, 0, 0, err, "position") && set_num(out, focus.pos as f64),
        b"count" => {
            if !arity(n, 1, 1, err, "count") {
                return false;
            }
            match require_nodeset(&args[0], "count", err) {
                Some(ns) => set_num(out, (*ns).count as f64),
                None => false,
            }
        }
        b"id" => arity(n, 1, 1, err, "id") && fn_id::<D>(ctx, args, out, err),
        b"local-name" => {
            if !arity(n, 0, 1, err, "local-name") {
                return false;
            }
            match name_target::<D>(args, focus, err, "local-name") {
                Some(t) => name_emit::<D>(t, false, out, err, "local-name"),
                None => false,
            }
        }
        b"name" => {
            if !arity(n, 0, 1, err, "name") {
                return false;
            }
            match name_target::<D>(args, focus, err, "name") {
                Some(t) => name_emit::<D>(t, true, out, err, "name"),
                None => false,
            }
        }
        b"namespace-uri" => {
            if !arity(n, 0, 1, err, "namespace-uri") {
                return false;
            }
            let t = match name_target::<D>(args, focus, err, "namespace-uri") {
                Some(t) => t,
                None => return false,
            };
            if D::is_null(t)
                || (D::node_type(t) != NTYPE_ELEMENT && D::node_type(t) != NTYPE_ATTRIBUTE)
                || !D::has_ns(t)
            {
                return set_string(out, b"", err, "namespace-uri");
            }
            let doc = D::doc_from_void(mkr_ctx_document(ctx));
            set_string(out, D::ns_uri(t, doc), err, "namespace-uri")
        }

        /* --- string --- */
        b"string" => {
            if !arity(n, 0, 1, err, "string") {
                return false;
            }
            match arg_or_self_text::<D>(focus, args, ctx, err) {
                Some(mut t) => {
                    /* transfer ownership: the value takes the allocation */
                    let owned = t.0;
                    t.0 = OwnedText { ptr: ptr::null_mut(), len: 0 };
                    (*out).type_ = T_STRING;
                    mkr_val_set_owned_text(out, owned);
                    true
                }
                None => false,
            }
        }
        b"concat" => {
            if n < 2 {
                err_setf!(err, XP_ERR_RUNTIME, "concat(): expected at least 2 arguments");
                return false;
            }
            fn_concat::<D>(ctx, args, out, err)
        }
        b"starts-with" => {
            if !arity(n, 2, 2, err, "starts-with") {
                return false;
            }
            two::<D, _>(ctx, args, err, |s, t| {
                *out = val_boolean(s.starts_with(t));
                true
            })
        }
        b"contains" => {
            if !arity(n, 2, 2, err, "contains") {
                return false;
            }
            two::<D, _>(ctx, args, err, |s, t| {
                *out = val_boolean(find_bytes(s, t).is_some());
                true
            })
        }
        b"substring-before" => {
            if !arity(n, 2, 2, err, "substring-before") {
                return false;
            }
            two::<D, _>(ctx, args, err, |s, t| {
                /* the bytes of s before the first t, or "" when t is empty or absent */
                let end = if t.is_empty() { 0 } else { find_bytes(s, t).unwrap_or(0) };
                set_string(out, &s[..end], err, "substring-before")
            })
        }
        b"substring-after" => {
            if !arity(n, 2, 2, err, "substring-after") {
                return false;
            }
            two::<D, _>(ctx, args, err, |s, t| {
                let rest: &[u8] = if t.is_empty() {
                    s
                } else {
                    match find_bytes(s, t) {
                        Some(i) => &s[i + t.len()..],
                        None => b"",
                    }
                };
                set_string(out, rest, err, "substring-after")
            })
        }
        b"substring" => arity(n, 2, 3, err, "substring") && fn_substring::<D>(ctx, args, out, err),
        b"string-length" => {
            if !arity(n, 0, 1, err, "string-length") {
                return false;
            }
            match arg_or_self_text::<D>(focus, args, ctx, err) {
                Some(t) => set_num(out, count_chars(t.as_slice()) as f64),
                None => false,
            }
        }
        b"normalize-space" => {
            arity(n, 0, 1, err, "normalize-space")
                && fn_normalize_space::<D>(ctx, focus, args, out, err)
        }
        b"translate" => arity(n, 3, 3, err, "translate") && fn_translate::<D>(ctx, args, out, err),

        /* --- boolean --- */
        b"not" => {
            arity(n, 1, 1, err, "not") && {
                *out = val_boolean(!val_to_boolean(&args[0]));
                true
            }
        }
        b"true" => {
            arity(n, 0, 0, err, "true") && {
                *out = val_boolean(true);
                true
            }
        }
        b"false" => {
            arity(n, 0, 0, err, "false") && {
                *out = val_boolean(false);
                true
            }
        }
        b"boolean" => {
            arity(n, 1, 1, err, "boolean") && {
                *out = val_boolean(val_to_boolean(&args[0]));
                true
            }
        }
        b"lang" => arity(n, 1, 1, err, "lang") && fn_lang::<D>(ctx, focus, args, out, err),

        /* --- number --- */
        b"number" => {
            if !arity(n, 0, 1, err, "number") {
                return false;
            }
            if n == 0 {
                /* number() with no argument is number(string(self)) */
                let mut t = Text::new();
                if !node_to_owned_text::<D>(focus.node, limits, err, t.as_mut()) {
                    return false;
                }
                return set_num(out, bytes_to_number(t.as_slice()));
            }
            match to_number::<D>(&args[0], ctx, err) {
                Some(d) => set_num(out, d),
                None => false,
            }
        }
        b"sum" => {
            if !arity(n, 1, 1, err, "sum") {
                return false;
            }
            let ns = match require_nodeset(&args[0], "sum", err) {
                Some(ns) => ns,
                None => return false,
            };
            let mut total = 0.0;
            for i in 0..(*ns).count {
                if mkr_limit_eval_op(limits, err) != 0 {
                    return false;
                }
                match cached_node_text::<D>(ctx, nodeset_at::<D>(ns, i), err) {
                    Some(s) => total += bytes_to_number(s),
                    None => return false,
                }
            }
            set_num(out, total)
        }
        b"floor" => num1::<D, _>(ctx, args, err, n, "floor", out, f64::floor),
        b"ceiling" => num1::<D, _>(ctx, args, err, n, "ceiling", out, f64::ceil),
        /* XPath round(): to the nearest integer, with .5 going toward +inf. */
        b"round" => num1::<D, _>(ctx, args, err, n, "round", out, |d| {
            if d.is_nan() {
                d
            } else {
                (d + 0.5).floor()
            }
        }),

        /* --- the CSS-lowered of-type hooks (XML only) --- */
        _ if D::IS_XML && local == FN_OF_TYPE_POS => {
            let doc = D::doc_from_void(mkr_ctx_document(ctx));
            set_num(out, of_type_pos::<D>(focus.node, true, doc))
        }
        _ if D::IS_XML && local == FN_OF_TYPE_POS_LAST => {
            let doc = D::doc_from_void(mkr_ctx_document(ctx));
            set_num(out, of_type_pos::<D>(focus.node, false, doc))
        }
        _ => false,
    }
}

unsafe fn set_num(out: *mut Val, d: f64) -> bool {
    *out = val_number(d);
    true
}

/// Pull both string operands, then run `f`. The guards free them on every path.
unsafe fn two<D: Dom, F>(ctx: *mut Context, args: &[Val], err: *mut Error, f: F) -> bool
where
    F: FnOnce(&[u8], &[u8]) -> bool,
{
    let a = match to_text::<D>(&args[0], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    let b = match to_text::<D>(&args[1], ctx, err) {
        Some(t) => t,
        None => return false,
    };
    f(a.as_slice(), b.as_slice())
}

unsafe fn num1<D: Dom, F>(
    ctx: *mut Context,
    args: &[Val],
    err: *mut Error,
    n: usize,
    name: &str,
    out: *mut Val,
    f: F,
) -> bool
where
    F: FnOnce(f64) -> f64,
{
    if !arity(n, 1, 1, err, name) {
        return false;
    }
    match to_number::<D>(&args[0], ctx, err) {
        Some(d) => set_num(out, f(d)),
        None => false,
    }
}

extern "C" {
    fn mkr_str_alloc(n: usize) -> *mut c_char;
}
