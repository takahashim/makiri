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
use super::eval::Evaluation;
use super::order::nodeset_unique_sorted;
use super::value::Focus;
use super::value::*;
use crate::err_setf;
use crate::falloc::Reserve;

/// Names the CSS lowering EMITS and the evaluator RESOLVES for an untyped
/// `:*-of-type`, where the "type" is the element's own expanded name - a
/// self-reference XPath 1.0 cannot express (there is no `current()`). The
/// leading \x01 cannot come out of the lexer, so these are unreachable from a
/// user expression. XML host only.
///
/// They live here, not beside the evaluator: one end emits them and the other
/// resolves them, and a name that only one end knows is a call that resolves to
/// nothing.
pub const FN_OF_TYPE_POS: &[u8] = b"\x01of-type-pos";
pub const FN_OF_TYPE_POS_LAST: &[u8] = b"\x01of-type-pos-last";

/// Namespace URI registered from Nokogiri's XPath context, so prefixed names
/// like "nokogiri-builtin:css-class" resolve.
pub const NS_NOKOGIRI_BUILTIN_URI: &[u8] = b"https://www.nokogiri.org/default_ns/ruby/builtins";

/// A builtin step: the value, or proof its error was written to the context's
/// budget.
pub type FnResult<T = ()> = Result<T, Reported>;

/// What a builtin returns: its result, owned, so a caller that fails after
/// receiving it still clears it.
pub type Answer<N> = FnResult<Val<N>>;

/// Every built-in has this shape (the C's `mkr_func_impl_t`). The engine owns
/// `args` and clears them after the call.
pub type FnImpl<'e, D> = unsafe fn(
    &mut Evaluation<'e, D>,
    &Focus<'e, D>,
    &[Val<<D as Dom<'e>>::Node>],
) -> Answer<<D as Dom<'e>>::Node>;

/// The built-in named `(ns_uri, local)`, or None - in which case the evaluator
/// routes the call to the registered resolver.
pub fn lookup<'e, D: Dom<'e>>(ns_uri: Option<&[u8]>, local: &[u8]) -> Option<FnImpl<'e, D>> {
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
fn require_nodeset<'v, N>(arg: &'v Val<N>, fname: &str, err: ErrSink) -> FnResult<&'v NodeSet<N>> {
    match arg.as_nodeset() {
        Some(ns) => Ok(ns),
        None => Err(err_setf!(
            err,
            XP_ERR_TYPE,
            "{}(): argument must be a node-set",
            fname
        )),
    }
}

/// An owned copy of `s`, or `Err` with `*err` naming `what` on OOM.
fn c_string(s: &[u8], err: ErrSink, what: &str) -> FnResult<Text> {
    Text::try_copy(s).ok_or_else(|| err_setf!(err, XP_ERR_OOM, "out of memory in {}()", what))
}

/// A string answer copied from `s`.
unsafe fn string<N>(s: &[u8], err: ErrSink, what: &str) -> Answer<N> {
    Ok(Val::string(c_string(s, err, what)?))
}

fn number<N>(d: f64) -> Answer<N> {
    Ok(Val::number(d))
}

fn boolean<N>(b: bool) -> Answer<N> {
    Ok(Val::boolean(b))
}

unsafe fn to_text<'e, D: Dom<'e>>(v: &Val<D::Node>, ev: &mut Evaluation<'e, D>) -> FnResult<Text> {
    let doc = ev.doc;
    val_to_owned_text_or_fail::<D>(doc, v, &mut ev.budget)
}

unsafe fn to_number<'e, D: Dom<'e>>(v: &Val<D::Node>, ev: &mut Evaluation<'e, D>) -> FnResult<f64> {
    let doc = ev.doc;
    val_to_number_or_fail::<D>(doc, v, &mut ev.budget)
}

/// The string-value of `args[0]`, or of the context node when there is none -
/// the idiom string() / string-length() / normalize-space() share.
unsafe fn arg_or_self_text<'e, D: Dom<'e>>(
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
    ev: &mut Evaluation<'e, D>,
) -> FnResult<Text> {
    match args.first() {
        Some(a) => to_text::<D>(a, ev),
        None => self_text::<D>(focus, ev),
    }
}

/// The string-value of the context node, or "" when there is none.
unsafe fn self_text<'e, D: Dom<'e>>(
    focus: &Focus<'e, D>,
    ev: &mut Evaluation<'e, D>,
) -> FnResult<Text> {
    match focus.node {
        Some(n) => node_to_owned_text::<D>(ev.doc, n, Some(&mut ev.budget)),
        None => owned_copy(
            b"",
            ev.budget.sink(),
            c"out of memory building node string-value",
        ),
    }
}

/// Pull both string operands, then run `f`. The guards free them on every path.
unsafe fn two<'e, D: Dom<'e>, F>(
    ev: &mut Evaluation<'e, D>,
    args: &[Val<D::Node>],
    f: F,
) -> Answer<D::Node>
where
    F: FnOnce(&[u8], &[u8]) -> Answer<D::Node>,
{
    let a = to_text::<D>(&args[0], ev)?;
    let b = to_text::<D>(&args[1], ev)?;
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

unsafe fn fn_last<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err, "last")?;
    number(focus.size as f64)
}

unsafe fn fn_position<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err, "position")?;
    number(focus.pos as f64)
}

unsafe fn fn_count<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, "count")?;
    let ns = require_nodeset(&args[0], "count", err)?;
    number(ns.len() as f64)
}

/// Walk the tree for an element whose `id` attribute is `id`.
///
/// Every visited node is charged to the op budget: without it, id() over a large
/// node-set - a token per node, a tree walk per token - drives quadratic work at
/// no cost. Returns Err on an overrun, with the budget's slot set.
unsafe fn find_by_id<'e, D: Dom<'e>>(
    doc: D,
    root: D::Node,
    id: &[u8],
    budget: &mut Budget,
) -> Result<Option<D::Node>, Reported> {
    if id.is_empty() {
        return Ok(None);
    }
    let mut n = root;
    loop {
        budget.charge_op()?;
        if doc.node_type(n) == NTYPE_ELEMENT && doc.get_attribute(n, b"id") == Some(id) {
            return Ok(Some(n));
        }
        if let Some(c) = doc.first_child(n) {
            n = c;
            continue;
        }
        loop {
            if n == root {
                return Ok(None);
            }
            if let Some(s) = doc.next(n) {
                n = s;
                break;
            }
            match doc.parent(n) {
                Some(p) => n = p,
                None => return Ok(None),
            }
        }
    }
}

/// Look up every whitespace-separated token of `s` and push each hit.
///
/// Duplicates go in unconditionally: the caller dedups the whole result with one
/// sort plus an adjacent pass, which beats a contains() check per insert.
unsafe fn id_collect<'e, D: Dom<'e>>(
    s: &[u8],
    root: D::Node,
    out: &mut NodeSet<D::Node>,
    ev: &mut Evaluation<'e, D>,
) -> FnResult {
    let doc = ev.doc;
    let budget = &mut ev.budget;
    for tok in s.split(|&b| super::lex::is_ws(b)).filter(|t| !t.is_empty()) {
        if let Some(hit) = find_by_id::<D>(doc, root, tok, budget)? {
            out.push(hit, budget)?;
        }
    }
    Ok(())
}

unsafe fn fn_id<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, "id")?;

    if D::IS_XML {
        /* Host policy: in XML an ID is an attribute DECLARED ID-typed by the
         * DTD, not any attribute named "id". DTDs are rejected at parse, so a
         * document read here carries no ID-typed attributes and id() is the
         * empty node-set. (xml:id is a separate, optional spec.) */
        return Ok(Val::default());
    }
    if ev.cx.document().is_null() {
        return Ok(Val::default());
    }
    let doc = ev.doc;
    let root = doc.document_node();
    /* Collected in a guard, so a failure part-way frees what was found. */
    let mut found = NodeSet::new();

    /* §4.1: a node-set argument treats each node's string-value as IDREFS;
     * anything else is converted to a string and split the same way. */
    if let Some(set) = args[0].as_nodeset() {
        (0..set.len()).try_for_each(|i| {
            let t = node_to_owned_text::<D>(doc, set.get(i), Some(&mut ev.budget))?;
            id_collect::<D>(t.as_slice(), root, &mut found, ev)
        })?;
    } else {
        let t = to_text::<D>(&args[0], ev)?;
        id_collect::<D>(t.as_slice(), root, &mut found, ev)?;
    }
    /* §4.1: the result is in document order with duplicates removed. */
    nodeset_unique_sorted::<D>(ev, &mut found);
    Ok(Val::nodeset(found))
}

/* ---------- name functions ---------- */

/// The first node of a node-set argument, or the context node when there is no
/// argument. A type error sets `*err`; an empty node-set yields a null handle,
/// which the callers render as "".
unsafe fn name_target<'e, D: Dom<'e>>(
    args: &[Val<D::Node>],
    focus: &Focus<'e, D>,
    err: ErrSink,
    fname: &str,
) -> FnResult<Option<D::Node>> {
    if args.is_empty() {
        return Ok(focus.node);
    }
    let ns = require_nodeset(&args[0], fname, err)?;
    if ns.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ns.get(0)))
    }
}

/// `n`'s local or qualified name as a string result; anything that is not an
/// element, attribute or PI yields "". A PI's name is its target either way (its
/// expanded-name is (null, target)). In HTML the qualified name equals the local
/// name, which also keeps the LXB_NS_HTML prefix out of the result.
unsafe fn name_emit<'e, D: Dom<'e>>(
    doc: D,
    n: Option<D::Node>,
    qualified: bool,
    err: ErrSink,
    fname: &str,
) -> Answer<D::Node> {
    let Some(n) = n else {
        return string(b"", err, fname);
    };
    let name: &[u8] = if let Some(a) = doc.as_attr(n) {
        if qualified {
            doc.attr_qualified_name(a)
        } else {
            doc.attr_local_name(a)
        }
    } else {
        match doc.node_type(n) {
            NTYPE_ELEMENT => {
                if qualified {
                    doc.qualified_name(n)
                } else {
                    doc.local_name(n)
                }
            }
            NTYPE_PI => doc.pi_name(n),
            _ => b"",
        }
    };
    string(name, err, fname)
}

unsafe fn fn_local_name<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 0, 1, err, "local-name")?;
    let t = name_target::<D>(args, focus, err, "local-name")?;
    name_emit::<D>(doc, t, false, err, "local-name")
}

unsafe fn fn_name<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 0, 1, err, "name")?;
    let t = name_target::<D>(args, focus, err, "name")?;
    name_emit::<D>(doc, t, true, err, "name")
}

unsafe fn fn_namespace_uri<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err, "namespace-uri")?;
    let doc = ev.doc;
    let Some(t) = name_target::<D>(args, focus, err, "namespace-uri")? else {
        return string(b"", err, "namespace-uri");
    };
    if (doc.node_type(t) != NTYPE_ELEMENT && doc.node_type(t) != NTYPE_ATTRIBUTE) || !doc.has_ns(t)
    {
        return string(b"", err, "namespace-uri");
    }
    string(doc.ns_uri(t), err, "namespace-uri")
}

/* ---------- string functions ---------- */

unsafe fn fn_string<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err, "string")?;
    Ok(Val::string(arg_or_self_text::<D>(focus, args, ev)?))
}

unsafe fn fn_concat<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    if args.len() < 2 {
        return Err(err_setf!(
            err,
            XP_ERR_RUNTIME,
            "concat(): expected at least 2 arguments"
        ));
    }
    let mut parts = try_vec::<Text>(args.len(), err, "concat")?;
    let mut total = 0usize;
    for a in args {
        let t = to_text::<D>(a, ev)?;
        total = match total.checked_add(t.as_slice().len()) {
            Some(n) => n,
            None => return Err(err_setf!(err, XP_ERR_OOM, "concat() size overflow")),
        };
        ev.budget.check_string_bytes(total)?;
        parts.push(t);
    }
    let joined = Text::try_fill(total, |dst| {
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
    Ok(Val::string(joined))
}

unsafe fn fn_starts_with<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err, "starts-with")?;
    two::<D, _>(ev, args, |s, t| boolean(s.starts_with(t)))
}

unsafe fn fn_contains<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err, "contains")?;
    two::<D, _>(ev, args, |s, t| boolean(find_bytes(s, t).is_some()))
}

unsafe fn fn_substring_before<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err, "substring-before")?;
    two::<D, _>(ev, args, |s, t| {
        /* the bytes of s before the first t, or "" when t is empty or absent */
        let end = if t.is_empty() {
            0
        } else {
            find_bytes(s, t).unwrap_or(0)
        };
        string(&s[..end], err, "substring-before")
    })
}

unsafe fn fn_substring_after<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err, "substring-after")?;
    two::<D, _>(ev, args, |s, t| {
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
unsafe fn fn_substring<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 3, err, "substring")?;
    let s = to_text::<D>(&args[0], ev)?;
    let start_d = to_number::<D>(&args[1], ev)?;
    let bytes = s.as_slice();
    let nchars = count_chars(bytes);
    let end_d = match args.get(2) {
        Some(a) => start_d + to_number::<D>(a, ev)?,
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

unsafe fn fn_string_length<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err, "string-length")?;
    let t = arg_or_self_text::<D>(focus, args, ev)?;
    number(count_chars(t.as_slice()) as f64)
}

/// normalize-space: collapse runs of whitespace and trim the ends.
unsafe fn fn_normalize_space<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err, "normalize-space")?;
    let s = arg_or_self_text::<D>(focus, args, ev)?;
    let src = s.as_slice();
    let normalized = Text::try_fill(src.len(), |dst| {
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
    Ok(Val::string(normalized))
}

/// translate(s, from, to) works on CHARACTERS, not bytes: each code point of `s`
/// that appears in `from` becomes the code point at the same position in `to`,
/// or is dropped when `from` is longer.
///
/// The input is valid UTF-8 (the literal lexer validates, and DOM string-values
/// are valid), but a decode failure fails closed rather than truncating.
unsafe fn fn_translate<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 3, 3, err, "translate")?;
    let mut texts = try_vec::<Text>(3, err, "translate")?;
    for a in args {
        texts.push(to_text::<D>(a, ev)?);
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
    let mut buf = Buf::new(ev.budget.limits.max_string_bytes);
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
                        ev.budget.limits.max_string_bytes
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
    Ok(Val::string(Text::from_buf(owned)))
}

/* ---------- boolean functions ---------- */

unsafe fn fn_not<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, "not")?;
    boolean(!val_to_boolean(&args[0]))
}

unsafe fn fn_true<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err, "true")?;
    boolean(true)
}

unsafe fn fn_false<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err, "false")?;
    boolean(false)
}

unsafe fn fn_boolean<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, "boolean")?;
    boolean(val_to_boolean(&args[0]))
}

unsafe fn fn_lang<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 1, 1, err, "lang")?;
    let want = to_text::<D>(&args[0], ev)?;
    let want = want.as_slice();
    /* Walk the ancestors for the host's language attribute. Host policy: XPath
     * 1.0 lang() is xml:lang based; HTML uses `lang`, accepting xml:lang as a
     * fallback. */
    let mut p = focus.node;
    while let Some(n) = p {
        if doc.node_type(n) == NTYPE_ELEMENT {
            let v = if D::IS_XML {
                doc.get_attribute(n, b"xml:lang")
            } else {
                doc.get_attribute(n, b"lang")
                    .or_else(|| doc.get_attribute(n, b"xml:lang"))
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
        p = doc.parent(n);
    }
    boolean(false)
}

/* ---------- number functions ---------- */

unsafe fn fn_number<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err, "number")?;
    match args.first() {
        Some(a) => number(to_number::<D>(a, ev)?),
        None => {
            /* number() with no argument is number(string(self)). */
            let t = self_text::<D>(focus, ev)?;
            number(bytes_to_number(t.as_slice()))
        }
    }
}

unsafe fn fn_sum<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, "sum")?;
    let ns = require_nodeset(&args[0], "sum", err)?;
    let mut total = 0.0;
    for i in 0..ns.len() {
        ev.budget.charge_op()?;
        total += cached_node_number::<D>(ev, ns.get(i))?;
    }
    number(total)
}

unsafe fn num1<'e, D: Dom<'e>, F>(
    ev: &mut Evaluation<'e, D>,
    args: &[Val<D::Node>],
    name: &str,
    f: F,
) -> Answer<D::Node>
where
    F: FnOnce(f64) -> f64,
{
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err, name)?;
    number(f(to_number::<D>(&args[0], ev)?))
}

unsafe fn fn_floor<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, "floor", f64::floor)
}

unsafe fn fn_ceiling<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, "ceiling", f64::ceil)
}

/// XPath round(): the integer closest to the argument, the one nearer +inf when
/// two are equally close.
///
/// XPath 1.0 §4.4 also fixes the signed zeros: NaN, the infinities and either
/// zero come back unchanged, and an argument in [-0.5, 0) rounds to negative
/// zero. `(d + 0.5).floor()` got both wrong - -0.5 gave +0, and
/// 0.49999999999999994 gave 1, because the addition rounds up to 1.0 - so the
/// distance to the floor is measured instead, which is exact.
fn round_half_up(d: f64) -> f64 {
    if d.is_nan() || d.is_infinite() || d == 0.0 {
        d
    } else if (-0.5..0.0).contains(&d) {
        -0.0
    } else {
        let floor = d.floor();
        if d - floor >= 0.5 {
            floor + 1.0
        } else {
            floor
        }
    }
}

unsafe fn fn_round<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, "round", round_half_up)
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

unsafe fn fn_css_class<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    _focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err, "nokogiri-builtin:css-class")?;
    two::<D, _>(ev, args, |hay, needle| {
        boolean(ws_token_match(Some(hay), Some(needle)))
    })
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
unsafe fn fn_local_name_is<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 1, 1, err, "nokogiri-builtin:local-name-is")?;
    let want = to_text::<D>(&args[0], ev)?;
    boolean(
        focus
            .node
            .is_some_and(|n| doc.qualified_name(n) == want.as_slice()),
    )
}

/* ---------- the CSS-lowered of-type hooks (XML only) ---------- */

/// Two elements are the same "type" iff they share an expanded name: local name
/// plus namespace URI.
fn same_type<'e, D: Dom<'e>>(a: D::Node, b: D::Node, doc: D) -> bool {
    doc.local_name(a) == doc.local_name(b) && doc.ns_uri(a) == doc.ns_uri(b)
}

/// The 1-based position of `node` among its same-type element siblings: forward
/// counts the preceding siblings, otherwise the following ones (from the end).
fn of_type_pos<'e, D: Dom<'e>>(node: Option<D::Node>, forward: bool, doc: D) -> f64 {
    let Some(node) = node else {
        return 0.0;
    };
    if doc.node_type(node) != NTYPE_ELEMENT {
        return 0.0;
    }
    let step = |n: D::Node| {
        if forward {
            doc.prev(n)
        } else {
            doc.next(n)
        }
    };
    let mut pos = 1i64;
    let mut s = step(node);
    while let Some(n) = s {
        if doc.node_type(n) == NTYPE_ELEMENT && same_type::<D>(node, n, doc) {
            pos += 1;
        }
        s = step(n);
    }
    pos as f64
}

unsafe fn fn_of_type_pos<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, true, doc))
}

unsafe fn fn_of_type_pos_last<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    focus: &Focus<'e, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, false, doc))
}

pub use crate::falloc::cstr::str_alloc;
