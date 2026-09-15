//! The built-in XPath 1.0 function library (mkr_xpath_funcs_body.h), plus the
//! two Nokogiri-compatible builtins and the CSS lowering's internal hooks.
//!
//! One function per builtin behind one signature, and `lookup` is the only place
//! that knows which names exist - the shape the C has as `fn_table` plus
//! `mkr_lookup_function`. Keeping "does it exist" and "what does it do" as one
//! question matters here: the evaluator asks before deciding whether to route a
//! call to a Ruby handler, so a second list of names maintained separately could
//! disagree with this one and turn an unknown function into a bare failure.
//!
//! The host-policy branches the C spells `#ifdef MKR_HOST_XML` are `D::IS_XML`.

use super::abi::*;
use super::dom::*;
use super::order::nodeset_unique_sorted;
use super::own::{OwnedText, OwnedVal, Set};
use super::value::Focus;
use super::value::*;
use crate::err_setf;
use crate::falloc::Reserve;

/// Namespace URI registered from Nokogiri's XPath context, so prefixed names
/// like "nokogiri-builtin:css-class" resolve.
pub const NS_NOKOGIRI_BUILTIN_URI: &[u8] = b"https://www.nokogiri.org/default_ns/ruby/builtins";

/// The internal of-type position names, declared in `xpath_abi` so the CSS
/// lowering that EMITS them and this evaluator that RESOLVES them cannot drift -
/// they sit in different feature trees.
pub use crate::xpath_abi::{FN_OF_TYPE_POS, FN_OF_TYPE_POS_LAST};

/// A builtin step: the value, or proof its error was written to `err`.
pub type FnResult<T = ()> = Result<T, Reported>;

/// What a builtin returns: its result, owned, so a caller that fails after
/// receiving it still clears it.
pub type Answer = FnResult<OwnedVal>;

/// Every built-in has this shape (the C's `mkr_func_impl_t`). The engine owns
/// `args` and clears them after the call.
pub type FnImpl<D> = unsafe fn(*mut Context, &Focus<D>, &[Val], ErrSink) -> Answer;

/// The built-in named `(ns_uri, local)`, or None - in which case the evaluator
/// routes the call to the registered resolver.
pub fn lookup<D: Dom>(ns_uri: Option<&[u8]>, local: &[u8]) -> Option<FnImpl<D>> {
    if let Some(uri) = ns_uri {
        /* The Nokogiri-compatible builtins live in one namespace; any other
         * registered namespace means a user-defined function, so it goes to the
         * resolver. */
        if uri != NS_NOKOGIRI_BUILTIN_URI {
            return None;
        }
        return Some(match local {
            b"css-class" => fn_css_class::<D>,
            b"local-name-is" => fn_local_name_is::<D>,
            _ => return None,
        });
    }
    /* The default namespace: the XPath 1.0 standard library. */
    Some(match local {
        /* node-set */
        b"last" => fn_last::<D>,
        b"position" => fn_position::<D>,
        b"count" => fn_count::<D>,
        b"id" => fn_id::<D>,
        b"local-name" => fn_local_name::<D>,
        b"namespace-uri" => fn_namespace_uri::<D>,
        b"name" => fn_name::<D>,
        /* string */
        b"string" => fn_string::<D>,
        b"concat" => fn_concat::<D>,
        b"starts-with" => fn_starts_with::<D>,
        b"contains" => fn_contains::<D>,
        b"substring-before" => fn_substring_before::<D>,
        b"substring-after" => fn_substring_after::<D>,
        b"substring" => fn_substring::<D>,
        b"string-length" => fn_string_length::<D>,
        b"normalize-space" => fn_normalize_space::<D>,
        b"translate" => fn_translate::<D>,
        /* boolean */
        b"not" => fn_not::<D>,
        b"true" => fn_true::<D>,
        b"false" => fn_false::<D>,
        b"boolean" => fn_boolean::<D>,
        b"lang" => fn_lang::<D>,
        /* number */
        b"number" => fn_number::<D>,
        b"sum" => fn_sum::<D>,
        b"floor" => fn_floor::<D>,
        b"ceiling" => fn_ceiling::<D>,
        b"round" => fn_round::<D>,
        /* the CSS lowering's internal hooks */
        _ if D::IS_XML && local == FN_OF_TYPE_POS => fn_of_type_pos::<D>,
        _ if D::IS_XML && local == FN_OF_TYPE_POS_LAST => fn_of_type_pos_last::<D>,
        _ => return None,
    })
}

/* ---------- shared helpers ---------- */

unsafe fn arity(got: usize, min: usize, max: usize, err: ErrSink, name: &str) -> FnResult {
    if got < min || got > max {
        return Err(if min == max {
            err_setf!(
                err,
                XP_ERR_RUNTIME,
                "{}(): expected {} argument(s), got {}",
                name,
                min,
                got
            )
        } else {
            err_setf!(
                err,
                XP_ERR_RUNTIME,
                "{}(): expected {}-{} argument(s), got {}",
                name,
                min,
                max,
                got
            )
        });
    }
    Ok(())
}

/// The shared "argument must be a node-set" check.
unsafe fn require_nodeset(arg: *const Val, fname: &str, err: ErrSink) -> FnResult<*const NodeSet> {
    match (*arg).as_nodeset() {
        Some(ns) => Ok(ns as *const NodeSet),
        None => Err(err_setf!(
            err,
            XP_ERR_TYPE,
            "{}(): argument must be a node-set",
            fname
        )),
    }
}

/// An owned copy of `s`, or `Err` with `*err` naming `what` on OOM.
unsafe fn c_string(s: &[u8], err: ErrSink, what: &str) -> FnResult<TextSlot> {
    TextSlot::try_copy_bytes(s, ErrSink::silent(), None)
        .map_err(|_| err_setf!(err, XP_ERR_OOM, "out of memory in {}()", what))
}

/// A string answer copied from `s`.
unsafe fn string(s: &[u8], err: ErrSink, what: &str) -> Answer {
    Ok(Val::string(c_string(s, err, what)?).into())
}

fn number(d: f64) -> Answer {
    Ok(Val::number(d).into())
}

fn boolean(b: bool) -> Answer {
    Ok(Val::boolean(b).into())
}

unsafe fn to_text<D: Dom>(v: *const Val, ctx: *mut Context, err: ErrSink) -> FnResult<OwnedText> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let mut t = OwnedText::new();
    val_to_owned_text_or_fail::<D>(doc, v, mkr_ctx_limits(ctx), err, t.as_mut())?;
    Ok(t)
}

unsafe fn to_number<D: Dom>(v: *const Val, ctx: *mut Context, err: ErrSink) -> FnResult<f64> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let mut d = 0.0;
    val_to_number_or_fail::<D>(doc, v, mkr_ctx_limits(ctx), err, &mut d)?;
    Ok(d)
}

/// The string-value of `args[0]`, or of the context node when there is none -
/// the idiom string() / string-length() / normalize-space() share.
unsafe fn arg_or_self_text<D: Dom>(
    focus: &Focus<D>,
    args: &[Val],
    ctx: *mut Context,
    err: ErrSink,
) -> FnResult<OwnedText> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    match args.first() {
        Some(a) => to_text::<D>(a, ctx, err),
        None => {
            let mut t = OwnedText::new();
            node_to_owned_text::<D>(doc, focus.node, mkr_ctx_limits(ctx), err, t.as_mut())?;
            Ok(t)
        }
    }
}

/// Pull both string operands, then run `f`. The guards free them on every path.
unsafe fn two<D: Dom, F>(ctx: *mut Context, args: &[Val], err: ErrSink, f: F) -> Answer
where
    F: FnOnce(&[u8], &[u8]) -> Answer,
{
    let a = to_text::<D>(&args[0], ctx, err)?;
    let b = to_text::<D>(&args[1], ctx, err)?;
    f(a.as_slice(), b.as_slice())
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

/// A `Vec` sized up front, so a failed allocation is an XPath OOM rather than
/// the abort a plain `Vec` growth would give under `panic = "abort"`.
fn try_vec<T>(n: usize, err: ErrSink, what: &str) -> FnResult<Vec<T>> {
    let mut v: Vec<T> = Vec::new();
    if v.mkr_reserve_exact(n).is_err() {
        return Err(err_setf!(err, XP_ERR_OOM, "out of memory in {}()", what));
    }
    Ok(v)
}

/* ---------- node-set functions ---------- */

unsafe fn fn_last<D: Dom>(
    _ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 0, err, "last")?;
    number(focus.size as f64)
}

unsafe fn fn_position<D: Dom>(
    _ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 0, err, "position")?;
    number(focus.pos as f64)
}

unsafe fn fn_count<D: Dom>(
    _ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 1, 1, err, "count")?;
    let ns = require_nodeset(&args[0], "count", err)?;
    number((*ns).count as f64)
}

/// Walk the tree for an element whose `id` attribute is `id`.
///
/// Every visited node is charged to the op budget: without it, id() over a large
/// node-set - a token per node, a tree walk per token - drives quadratic work at
/// no cost. Returns Err on an overrun, with `*err` set.
unsafe fn find_by_id<D: Dom>(
    doc: D::Doc,
    root: D::Node,
    id: &[u8],
    limits: *mut Limits,
    err: ErrSink,
) -> Result<D::Node, Reported> {
    if D::is_null(root) || id.is_empty() {
        return Ok(D::null());
    }
    let mut n = root;
    while !D::is_null(n) {
        mkr_limit_eval_op(limits, err)?;
        if D::node_type(doc, n) == NTYPE_ELEMENT && D::get_attribute(doc, n, b"id") == Some(id) {
            return Ok(n);
        }
        if !D::is_null(D::first_child(doc, n)) {
            n = D::first_child(doc, n);
        } else {
            while !D::is_null(n) && n != root && D::is_null(D::next(doc, n)) {
                n = D::parent(doc, n);
            }
            if D::is_null(n) || n == root {
                break;
            }
            n = D::next(doc, n);
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
    err: ErrSink,
) -> FnResult {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let limits = mkr_ctx_limits(ctx);
    for tok in s.split(|&b| super::lex::is_ws(b)).filter(|t| !t.is_empty()) {
        let hit = find_by_id::<D>(doc, root, tok, limits, err)?;
        if !D::is_null(hit) {
            mkr_nodeset_push(out, D::to_void(hit), limits, err)?;
        }
    }
    Ok(())
}

unsafe fn fn_id<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 1, 1, err, "id")?;

    if D::IS_XML {
        /* Host policy: in XML an ID is an attribute DECLARED ID-typed by the
         * DTD, not any attribute named "id". DTDs are rejected at parse, so a
         * document read here carries no ID-typed attributes and id() is the
         * empty node-set. (xml:id is a separate, optional spec.) */
        return Ok(OwnedVal::new());
    }
    let doc = mkr_ctx_document(ctx);
    if doc.is_null() {
        return Ok(OwnedVal::new());
    }
    let root = D::document_node(D::doc_from_void(doc));
    /* Collected in a guard, so a failure part-way frees what was found. */
    let mut found = Set::new();
    let ns_out = found.as_mut();

    /* §4.1: a node-set argument treats each node's string-value as IDREFS;
     * anything else is converted to a string and split the same way. */
    if let Some(set) = args[0].as_nodeset() {
        (0..set.count).try_for_each(|i| {
            let mut t = OwnedText::new();
            node_to_owned_text::<D>(
                D::doc_from_void(doc),
                nodeset_at::<D>(set, i),
                mkr_ctx_limits(ctx),
                err,
                t.as_mut(),
            )?;
            id_collect::<D>(t.as_slice(), root, ns_out, ctx, err)
        })?;
    } else {
        let t = to_text::<D>(&args[0], ctx, err)?;
        id_collect::<D>(t.as_slice(), root, ns_out, ctx, err)?;
    }
    /* §4.1: the result is in document order with duplicates removed. */
    nodeset_unique_sorted::<D>(ctx, found.as_mut());
    Ok(Val::nodeset(found.take()).into())
}

/* ---------- name functions ---------- */

/// The first node of a node-set argument, or the context node when there is no
/// argument. A type error sets `*err`; an empty node-set yields a null handle,
/// which the callers render as "".
unsafe fn name_target<D: Dom>(
    args: &[Val],
    focus: &Focus<D>,
    err: ErrSink,
    fname: &str,
) -> FnResult<D::Node> {
    if args.is_empty() {
        return Ok(focus.node);
    }
    let ns = require_nodeset(&args[0], fname, err)?;
    if (*ns).count == 0 {
        Ok(D::null())
    } else {
        Ok(nodeset_at::<D>(ns, 0))
    }
}

/// `n`'s local or qualified name as a string result; anything that is not an
/// element, attribute or PI yields "". A PI's name is its target either way (its
/// expanded-name is (null, target)). In HTML the qualified name equals the local
/// name, which also keeps the LXB_NS_HTML prefix out of the result.
unsafe fn name_emit<D: Dom>(
    doc: D::Doc,
    n: D::Node,
    qualified: bool,
    err: ErrSink,
    fname: &str,
) -> Answer {
    if D::is_null(n) {
        return string(b"", err, fname);
    }
    let name: &[u8] = match D::node_type(doc, n) {
        NTYPE_ATTRIBUTE => {
            if qualified {
                D::attr_qualified_name(doc, n)
            } else {
                D::attr_local_name(doc, n)
            }
        }
        NTYPE_ELEMENT => {
            if qualified {
                D::qualified_name(doc, n)
            } else {
                D::local_name(doc, n)
            }
        }
        NTYPE_PI => D::pi_name(doc, n),
        _ => b"",
    };
    string(name, err, fname)
}

unsafe fn fn_local_name<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    arity(args.len(), 0, 1, err, "local-name")?;
    let t = name_target::<D>(args, focus, err, "local-name")?;
    name_emit::<D>(doc, t, false, err, "local-name")
}

unsafe fn fn_name<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    arity(args.len(), 0, 1, err, "name")?;
    let t = name_target::<D>(args, focus, err, "name")?;
    name_emit::<D>(doc, t, true, err, "name")
}

unsafe fn fn_namespace_uri<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 1, err, "namespace-uri")?;
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let t = name_target::<D>(args, focus, err, "namespace-uri")?;
    if D::is_null(t)
        || (D::node_type(doc, t) != NTYPE_ELEMENT && D::node_type(doc, t) != NTYPE_ATTRIBUTE)
        || !D::has_ns(doc, t)
    {
        return string(b"", err, "namespace-uri");
    }
    string(D::ns_uri(doc, t), err, "namespace-uri")
}

/* ---------- string functions ---------- */

unsafe fn fn_string<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 1, err, "string")?;
    let mut t = arg_or_self_text::<D>(focus, args, ctx, err)?;
    Ok(Val::string(t.take()).into())
}

unsafe fn fn_concat<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    if args.len() < 2 {
        return Err(err_setf!(
            err,
            XP_ERR_RUNTIME,
            "concat(): expected at least 2 arguments"
        ));
    }
    let limits = mkr_ctx_limits(ctx);
    let mut parts = try_vec::<OwnedText>(args.len(), err, "concat")?;
    let mut total = 0usize;
    for a in args {
        let t = to_text::<D>(a, ctx, err)?;
        total = match total.checked_add(t.as_slice().len()) {
            Some(n) => n,
            None => return Err(err_setf!(err, XP_ERR_OOM, "concat() size overflow")),
        };
        mkr_limit_check_string_bytes(limits, total, err)?;
        parts.push(t);
    }
    let joined = TextSlot::try_fill(total, |dst| {
        let mut off = 0usize;
        for p in &parts {
            let s = p.as_slice();
            dst[off..off + s.len()].copy_from_slice(s);
            off += s.len();
        }
        off
    });
    let Some(joined) = joined else {
        return Err(err_setf!(err, XP_ERR_OOM, "out of memory in concat()"));
    };
    Ok(Val::string(joined).into())
}

unsafe fn fn_starts_with<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 2, err, "starts-with")?;
    two::<D, _>(ctx, args, err, |s, t| boolean(s.starts_with(t)))
}

unsafe fn fn_contains<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 2, err, "contains")?;
    two::<D, _>(ctx, args, err, |s, t| boolean(find_bytes(s, t).is_some()))
}

unsafe fn fn_substring_before<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 2, err, "substring-before")?;
    two::<D, _>(ctx, args, err, |s, t| {
        /* the bytes of s before the first t, or "" when t is empty or absent */
        let end = if t.is_empty() {
            0
        } else {
            find_bytes(s, t).unwrap_or(0)
        };
        string(&s[..end], err, "substring-before")
    })
}

unsafe fn fn_substring_after<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 2, err, "substring-after")?;
    two::<D, _>(ctx, args, err, |s, t| {
        let rest: &[u8] = if t.is_empty() {
            s
        } else {
            match find_bytes(s, t) {
                Some(i) => &s[i + t.len()..],
                None => b"",
            }
        };
        string(rest, err, "substring-after")
    })
}

/// substring(s, start[, length]). Positions are 1-based character offsets that
/// round to nearest, and out-of-range positions clip silently.
unsafe fn fn_substring<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 3, err, "substring")?;
    let s = to_text::<D>(&args[0], ctx, err)?;
    let start_d = to_number::<D>(&args[1], ctx, err)?;
    let bytes = s.as_slice();
    let nchars = count_chars(bytes);
    let end_d = match args.get(2) {
        Some(a) => start_d + to_number::<D>(a, ctx, err)?,
        None => nchars as f64 + 1.0,
    };

    if start_d.is_nan() || end_d.is_nan() {
        return string(b"", err, "substring");
    }
    /* Round, then clamp AS DOUBLES before any cast: start/end can be infinite
     * or beyond i64 (`substring(s, 1 div 0)`), where casting first would be
     * undefined in C and saturating here - either way not the spec's clip. */
    let imax = nchars as f64 + 1.0;
    let rstart = (start_d + 0.5).floor().clamp(1.0, imax);
    let rend = (end_d + 0.5).floor().clamp(1.0, imax);
    if rend <= rstart {
        return string(b"", err, "substring");
    }
    let from = advance_chars(bytes, (rstart as i64 - 1) as usize);
    let to = from + advance_chars(&bytes[from..], (rend as i64 - rstart as i64) as usize);
    string(&bytes[from..to], err, "substring")
}

unsafe fn fn_string_length<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 1, err, "string-length")?;
    let t = arg_or_self_text::<D>(focus, args, ctx, err)?;
    number(count_chars(t.as_slice()) as f64)
}

/// normalize-space: collapse runs of whitespace and trim the ends.
unsafe fn fn_normalize_space<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 1, err, "normalize-space")?;
    let s = arg_or_self_text::<D>(focus, args, ctx, err)?;
    let src = s.as_slice();
    let normalized = TextSlot::try_fill(src.len(), |dst| {
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
        w
    });
    let Some(normalized) = normalized else {
        return Err(err_setf!(
            err,
            XP_ERR_OOM,
            "out of memory in normalize-space()"
        ));
    };
    Ok(Val::string(normalized).into())
}

/// translate(s, from, to) works on CHARACTERS, not bytes: each code point of `s`
/// that appears in `from` becomes the code point at the same position in `to`,
/// or is dropped when `from` is longer.
///
/// The input is valid UTF-8 (the literal lexer validates, and DOM string-values
/// are valid), but a decode failure fails closed rather than truncating.
unsafe fn fn_translate<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 3, 3, err, "translate")?;
    let limits = mkr_ctx_limits(ctx);
    let mut texts = try_vec::<OwnedText>(3, err, "translate")?;
    for a in args {
        texts.push(to_text::<D>(a, ctx, err)?);
    }
    let (sv, fv, tv) = match (
        core::str::from_utf8(texts[0].as_slice()),
        core::str::from_utf8(texts[1].as_slice()),
        core::str::from_utf8(texts[2].as_slice()),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => {
            return Err(err_setf!(
                err,
                XP_ERR_RUNTIME,
                "invalid UTF-8 in translate() argument"
            ));
        }
    };
    /* A character is never shorter than a byte, so the byte length bounds the
     * count - reserving up front keeps a failed allocation an XPath OOM rather
     * than the abort a growing Vec would give under `panic = "abort"`. */
    let mut from_cp = try_vec::<char>(fv.len(), err, "translate")?;
    from_cp.extend(fv.chars());
    let mut to_cp = try_vec::<char>(tv.len(), err, "translate")?;
    to_cp.extend(tv.chars());

    /* Capped: a multibyte replacement can push the result past the limit even
     * when the input is inside it ("a" -> an emoji), so the append fails closed
     * with LIMIT or OOM. */
    let mut buf = Buf::new((*limits).max_string_bytes);
    let mut enc = [0u8; 4];
    for c in sv.chars() {
        let emit: Option<&str> = match from_cp.iter().position(|&f| f == c) {
            None => Some(c.encode_utf8(&mut enc)), /* not in `from`: keep it */
            Some(k) if k < to_cp.len() => Some(to_cp[k].encode_utf8(&mut enc)),
            Some(_) => None, /* past `to`: drop it */
        };
        if let Some(e) = emit {
            let result = buf.append(e.as_bytes());
            if result.is_err() {
                buf.free();
                return Err(if matches!(result, Err(crate::cbuf::BufError::Limit)) {
                    err_setf!(
                        err,
                        XP_ERR_LIMIT,
                        "string size limit exceeded ({} bytes) in translate()",
                        (*limits).max_string_bytes
                    )
                } else {
                    err_setf!(err, XP_ERR_OOM, "out of memory in translate()")
                });
            }
        }
    }
    let owned = match buf.steal() {
        Ok(owned) => owned,
        Err(_) => {
            return Err(err_setf!(err, XP_ERR_OOM, "out of memory in translate()"));
        }
    };
    Ok(Val::string(TextSlot::from_buf(owned)).into())
}

/* ---------- boolean functions ---------- */

unsafe fn fn_not<D: Dom>(
    _ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 1, 1, err, "not")?;
    boolean(!val_to_boolean(&args[0]))
}

unsafe fn fn_true<D: Dom>(
    _ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 0, err, "true")?;
    boolean(true)
}

unsafe fn fn_false<D: Dom>(
    _ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 0, 0, err, "false")?;
    boolean(false)
}

unsafe fn fn_boolean<D: Dom>(
    _ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 1, 1, err, "boolean")?;
    boolean(val_to_boolean(&args[0]))
}

unsafe fn fn_lang<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    arity(args.len(), 1, 1, err, "lang")?;
    let want = to_text::<D>(&args[0], ctx, err)?;
    let want = want.as_slice();
    /* Walk the ancestors for the host's language attribute. Host policy: XPath
     * 1.0 lang() is xml:lang based; HTML uses `lang`, accepting xml:lang as a
     * fallback. */
    let mut p = focus.node;
    while !D::is_null(p) {
        if D::node_type(doc, p) == NTYPE_ELEMENT {
            let v = if D::IS_XML {
                D::get_attribute(doc, p, b"xml:lang")
            } else {
                D::get_attribute(doc, p, b"lang").or_else(|| D::get_attribute(doc, p, b"xml:lang"))
            };
            if let Some(v) = v {
                /* Case-insensitive compare of the prefix up to a '-'. */
                if v.len() >= want.len()
                    && v[..want.len()].eq_ignore_ascii_case(want)
                    && (v.len() == want.len() || v[want.len()] == b'-')
                {
                    return boolean(true);
                }
            }
        }
        p = D::parent(doc, p);
    }
    boolean(false)
}

/* ---------- number functions ---------- */

unsafe fn fn_number<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    arity(args.len(), 0, 1, err, "number")?;
    match args.first() {
        Some(a) => number(to_number::<D>(a, ctx, err)?),
        None => {
            /* number() with no argument is number(string(self)). */
            let mut t = OwnedText::new();
            node_to_owned_text::<D>(doc, focus.node, mkr_ctx_limits(ctx), err, t.as_mut())?;
            number(bytes_to_number(t.as_slice()))
        }
    }
}

unsafe fn fn_sum<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 1, 1, err, "sum")?;
    let ns = require_nodeset(&args[0], "sum", err)?;
    let limits = mkr_ctx_limits(ctx);
    let mut total = 0.0;
    for i in 0..(*ns).count {
        mkr_limit_eval_op(limits, err)?;
        total += bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(ns, i), err)?);
    }
    number(total)
}

unsafe fn num1<D: Dom, F>(ctx: *mut Context, args: &[Val], err: ErrSink, name: &str, f: F) -> Answer
where
    F: FnOnce(f64) -> f64,
{
    arity(args.len(), 1, 1, err, name)?;
    number(f(to_number::<D>(&args[0], ctx, err)?))
}

unsafe fn fn_floor<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    num1::<D, _>(ctx, args, err, "floor", f64::floor)
}

unsafe fn fn_ceiling<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    num1::<D, _>(ctx, args, err, "ceiling", f64::ceil)
}

/// XPath round(): to the nearest integer, with .5 going toward +inf.
unsafe fn fn_round<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    num1::<D, _>(ctx, args, err, "round", |d| {
        if d.is_nan() {
            d
        } else {
            (d + 0.5).floor()
        }
    })
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

unsafe fn fn_css_class<D: Dom>(
    ctx: *mut Context,
    _focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    arity(args.len(), 2, 2, err, "nokogiri-builtin:css-class")?;
    two::<D, _>(ctx, args, err, |hay, needle| {
        boolean(ws_token_match(Some(hay), Some(needle)))
    })
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
unsafe fn fn_local_name_is<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    args: &[Val],
    err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    arity(args.len(), 1, 1, err, "nokogiri-builtin:local-name-is")?;
    let want = to_text::<D>(&args[0], ctx, err)?;
    boolean(!D::is_null(focus.node) && D::qualified_name(doc, focus.node) == want.as_slice())
}

/* ---------- the CSS-lowered of-type hooks (XML only) ---------- */

/// Two elements are the same "type" iff they share an expanded name: local name
/// plus namespace URI.
unsafe fn same_type<D: Dom>(a: D::Node, b: D::Node, doc: D::Doc) -> bool {
    D::local_name(doc, a) == D::local_name(doc, b) && D::ns_uri(doc, a) == D::ns_uri(doc, b)
}

/// The 1-based position of `node` among its same-type element siblings: forward
/// counts the preceding siblings, otherwise the following ones (from the end).
unsafe fn of_type_pos<D: Dom>(node: D::Node, forward: bool, doc: D::Doc) -> f64 {
    if D::is_null(node) || D::node_type(doc, node) != NTYPE_ELEMENT {
        return 0.0;
    }
    let step = |n: D::Node| {
        if forward {
            D::prev(doc, n)
        } else {
            D::next(doc, n)
        }
    };
    let mut pos = 1i64;
    let mut s = step(node);
    while !D::is_null(s) {
        if D::node_type(doc, s) == NTYPE_ELEMENT && same_type::<D>(node, s, doc) {
            pos += 1;
        }
        s = step(s);
    }
    pos as f64
}

unsafe fn fn_of_type_pos<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    _args: &[Val],
    _err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    number(of_type_pos::<D>(focus.node, true, doc))
}

unsafe fn fn_of_type_pos_last<D: Dom>(
    ctx: *mut Context,
    focus: &Focus<D>,
    _args: &[Val],
    _err: ErrSink,
) -> Answer {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    number(of_type_pos::<D>(focus.node, false, doc))
}

pub use crate::falloc::cstr::mkr_str_alloc;
