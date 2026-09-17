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

#![forbid(unsafe_code)]

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
pub type FnImpl<'e, 'd, D> = fn(
    &mut Evaluation<'e, 'd, D>,
    &Focus<'d, D>,
    &[Val<<D as Dom<'d>>::Node>],
) -> Answer<<D as Dom<'d>>::Node>;

/// The built-in named `(ns_uri, local)`, or None - in which case the evaluator
/// routes the call to the registered resolver.
pub fn lookup<'e, 'd, D: Dom<'d>>(ns_uri: Option<&[u8]>, local: &[u8]) -> Option<FnImpl<'e, 'd, D>> {
    if let Some(uri) = ns_uri {
        /* The Nokogiri-compatible builtins live in one namespace; any other
         * registered namespace means a user-defined function, so it goes to the
         * resolver. */
        if uri != NS_NOKOGIRI_BUILTIN_URI {
            return None;
        }
        let f: FnImpl<'e, 'd, D> = match local {
            b"css-class" => fn_css_class::<D> as FnImpl<'e, 'd, D>,
            b"local-name-is" => fn_local_name_is::<D> as FnImpl<'e, 'd, D>,
            _ => return None,
        };
        return Some(f);
    }
    /* The default namespace: the XPath 1.0 standard library. */
    let f: FnImpl<'e, 'd, D> = match local {
        /* node-set */
        b"last" => fn_last::<D> as FnImpl<'e, 'd, D>,
        b"position" => fn_position::<D> as FnImpl<'e, 'd, D>,
        b"count" => fn_count::<D> as FnImpl<'e, 'd, D>,
        b"id" => fn_id::<D> as FnImpl<'e, 'd, D>,
        b"local-name" => fn_local_name::<D> as FnImpl<'e, 'd, D>,
        b"namespace-uri" => fn_namespace_uri::<D> as FnImpl<'e, 'd, D>,
        b"name" => fn_name::<D> as FnImpl<'e, 'd, D>,
        /* string */
        b"string" => fn_string::<D> as FnImpl<'e, 'd, D>,
        b"concat" => fn_concat::<D> as FnImpl<'e, 'd, D>,
        b"starts-with" => fn_starts_with::<D> as FnImpl<'e, 'd, D>,
        b"contains" => fn_contains::<D> as FnImpl<'e, 'd, D>,
        b"substring-before" => fn_substring_before::<D> as FnImpl<'e, 'd, D>,
        b"substring-after" => fn_substring_after::<D> as FnImpl<'e, 'd, D>,
        b"substring" => fn_substring::<D> as FnImpl<'e, 'd, D>,
        b"string-length" => fn_string_length::<D> as FnImpl<'e, 'd, D>,
        b"normalize-space" => fn_normalize_space::<D> as FnImpl<'e, 'd, D>,
        b"translate" => fn_translate::<D> as FnImpl<'e, 'd, D>,
        /* boolean */
        b"not" => fn_not::<D> as FnImpl<'e, 'd, D>,
        b"true" => fn_true::<D> as FnImpl<'e, 'd, D>,
        b"false" => fn_false::<D> as FnImpl<'e, 'd, D>,
        b"boolean" => fn_boolean::<D> as FnImpl<'e, 'd, D>,
        b"lang" => fn_lang::<D> as FnImpl<'e, 'd, D>,
        /* number */
        b"number" => fn_number::<D> as FnImpl<'e, 'd, D>,
        b"sum" => fn_sum::<D> as FnImpl<'e, 'd, D>,
        b"floor" => fn_floor::<D> as FnImpl<'e, 'd, D>,
        b"ceiling" => fn_ceiling::<D> as FnImpl<'e, 'd, D>,
        b"round" => fn_round::<D> as FnImpl<'e, 'd, D>,
        /* the CSS lowering's internal hooks */
        _ if D::IS_XML && local == FN_OF_TYPE_POS => fn_of_type_pos::<D> as FnImpl<'e, 'd, D>,
        _ if D::IS_XML && local == FN_OF_TYPE_POS_LAST => fn_of_type_pos_last::<D> as FnImpl<'e, 'd, D>,
        _ => return None,
    };
    Some(f)
}

/* ---------- shared helpers ---------- */

fn arity(got: usize, min: usize, max: usize, err: ErrSink, name: &str) -> FnResult {
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
fn string<N>(s: &[u8], err: ErrSink, what: &str) -> Answer<N> {
    Ok(Val::string(c_string(s, err, what)?))
}

fn number<N>(d: f64) -> Answer<N> {
    Ok(Val::number(d))
}

fn boolean<N>(b: bool) -> Answer<N> {
    Ok(Val::boolean(b))
}

fn to_text<'e, 'd, D: Dom<'d>>(v: &Val<D::Node>, ev: &mut Evaluation<'e, 'd, D>) -> FnResult<Text> {
    let doc = ev.doc;
    val_to_owned_text_or_fail::<D>(doc, v, &mut ev.budget)
}

fn to_number<'e, 'd, D: Dom<'d>>(v: &Val<D::Node>, ev: &mut Evaluation<'e, 'd, D>) -> FnResult<f64> {
    let doc = ev.doc;
    val_to_number_or_fail::<D>(doc, v, &mut ev.budget)
}

/// The string-value of `args[0]`, or of the context node when there is none -
/// the idiom string() / string-length() / normalize-space() share.
fn arg_or_self_text<'e, 'd, D: Dom<'d>>(
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
    ev: &mut Evaluation<'e, 'd, D>,
) -> FnResult<Text> {
    match args.first() {
        Some(a) => to_text::<D>(a, ev),
        None => self_text::<D>(focus, ev),
    }
}

/// The string-value of the context node, or "" when there is none.
fn self_text<'e, 'd, D: Dom<'d>>(focus: &Focus<'d, D>, ev: &mut Evaluation<'e, 'd, D>) -> FnResult<Text> {
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
fn two<'e, 'd, D: Dom<'d>, F>(
    ev: &mut Evaluation<'e, 'd, D>,
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

fn fn_last<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err.clone(), "last")?;
    number(focus.size as f64)
}

fn fn_position<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err.clone(), "position")?;
    number(focus.pos as f64)
}

fn fn_count<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err.clone(), "count")?;
    let ns = require_nodeset(&args[0], "count", err)?;
    number(ns.len() as f64)
}

/// Walk the tree for an element whose `id` attribute is `id`.
///
/// Every visited node is charged to the op budget: without it, id() over a large
/// node-set - a token per node, a tree walk per token - drives quadratic work at
/// no cost. Returns Err on an overrun, with the budget's slot set.
fn find_by_id<'e, 'd, D: Dom<'d>>(
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
fn id_collect<'e, 'd, D: Dom<'d>>(
    s: &[u8],
    root: D::Node,
    out: &mut NodeSet<D::Node>,
    ev: &mut Evaluation<'e, 'd, D>,
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

fn fn_id<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err.clone(), "id")?;

    if D::IS_XML {
        /* Host policy: in XML an ID is an attribute DECLARED ID-typed by the
         * DTD, not any attribute named "id". DTDs are rejected at parse, so a
         * document read here carries no ID-typed attributes and id() is the
         * empty node-set. (xml:id is a separate, optional spec.) */
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
fn name_target<'e, 'd, D: Dom<'d>>(
    args: &[Val<D::Node>],
    focus: &Focus<'d, D>,
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
fn name_emit<'e, 'd, D: Dom<'d>>(
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

fn fn_local_name<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 0, 1, err.clone(), "local-name")?;
    let t = name_target::<D>(args, focus, err.clone(), "local-name")?;
    name_emit::<D>(doc, t, false, err.clone(), "local-name")
}

fn fn_name<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 0, 1, err.clone(), "name")?;
    let t = name_target::<D>(args, focus, err.clone(), "name")?;
    name_emit::<D>(doc, t, true, err.clone(), "name")
}

fn fn_namespace_uri<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err.clone(), "namespace-uri")?;
    let doc = ev.doc;
    let Some(t) = name_target::<D>(args, focus, err.clone(), "namespace-uri")? else {
        return string(b"", err.clone(), "namespace-uri");
    };
    if (doc.node_type(t) != NTYPE_ELEMENT && doc.node_type(t) != NTYPE_ATTRIBUTE) || !doc.has_ns(t)
    {
        return string(b"", err.clone(), "namespace-uri");
    }
    string(doc.ns_uri(t), err.clone(), "namespace-uri")
}

/* ---------- string functions ---------- */

fn fn_string<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err.clone(), "string")?;
    Ok(Val::string(arg_or_self_text::<D>(focus, args, ev)?))
}

fn fn_concat<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
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
    let mut parts = try_vec::<Text>(args.len(), err.clone(), "concat")?;
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

fn fn_starts_with<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "starts-with")?;
    two::<D, _>(ev, args, |s, t| boolean(s.starts_with(t)))
}

fn fn_contains<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "contains")?;
    two::<D, _>(ev, args, |s, t| boolean(find_bytes(s, t).is_some()))
}

fn fn_substring_before<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "substring-before")?;
    two::<D, _>(ev, args, |s, t| {
        /* the bytes of s before the first t, or "" when t is empty or absent */
        let end = if t.is_empty() {
            0
        } else {
            find_bytes(s, t).unwrap_or(0)
        };
        string(&s[..end], err.clone(), "substring-before")
    })
}

fn fn_substring_after<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "substring-after")?;
    two::<D, _>(ev, args, |s, t| {
        let rest: &[u8] = if t.is_empty() {
            s
        } else {
            match find_bytes(s, t) {
                Some(i) => &s[i + t.len()..],
                None => b"",
            }
        };
        string(rest, err.clone(), "substring-after")
    })
}

/// substring(s, start[, length]). Positions are 1-based character offsets that
/// round to nearest, and out-of-range positions clip silently.
fn fn_substring<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 3, err.clone(), "substring")?;
    let s = to_text::<D>(&args[0], ev)?;
    let start_d = to_number::<D>(&args[1], ev)?;
    let bytes = s.as_slice();
    let nchars = count_chars(bytes);
    let end_d = match args.get(2) {
        Some(a) => start_d + to_number::<D>(a, ev)?,
        None => nchars as f64 + 1.0,
    };

    if start_d.is_nan() || end_d.is_nan() {
        return string(b"", err.clone(), "substring");
    }
    /* Round, then clamp AS DOUBLES before any cast: start/end can be infinite
     * or beyond i64 (`substring(s, 1 div 0)`), where casting first would be
     * undefined in C and saturating here - either way not the spec's clip. */
    let imax = nchars as f64 + 1.0;
    let rstart = (start_d + 0.5).floor().clamp(1.0, imax);
    let rend = (end_d + 0.5).floor().clamp(1.0, imax);
    if rend <= rstart {
        return string(b"", err.clone(), "substring");
    }
    let from = advance_chars(bytes, (rstart as i64 - 1) as usize);
    let to = from + advance_chars(&bytes[from..], (rend as i64 - rstart as i64) as usize);
    string(&bytes[from..to], err.clone(), "substring")
}

fn fn_string_length<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err.clone(), "string-length")?;
    let t = arg_or_self_text::<D>(focus, args, ev)?;
    number(count_chars(t.as_slice()) as f64)
}

/// normalize-space: collapse runs of whitespace and trim the ends.
fn fn_normalize_space<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err.clone(), "normalize-space")?;
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
fn fn_translate<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 3, 3, err.clone(), "translate")?;
    let mut texts = try_vec::<Text>(3, err.clone(), "translate")?;
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
    let mut from_cp = try_vec::<char>(fv.len(), err.clone(), "translate")?;
    from_cp.extend(fv.chars());
    let mut to_cp = try_vec::<char>(tv.len(), err.clone(), "translate")?;
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

fn fn_not<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err.clone(), "not")?;
    boolean(!val_to_boolean(&args[0]))
}

fn fn_true<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err.clone(), "true")?;
    boolean(true)
}

fn fn_false<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 0, err.clone(), "false")?;
    boolean(false)
}

fn fn_boolean<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err.clone(), "boolean")?;
    boolean(val_to_boolean(&args[0]))
}

fn fn_lang<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 1, 1, err.clone(), "lang")?;
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

fn fn_number<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 0, 1, err.clone(), "number")?;
    match args.first() {
        Some(a) => number(to_number::<D>(a, ev)?),
        None => {
            /* number() with no argument is number(string(self)). */
            let t = self_text::<D>(focus, ev)?;
            number(bytes_to_number(t.as_slice()))
        }
    }
}

fn fn_sum<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 1, 1, err.clone(), "sum")?;
    let ns = require_nodeset(&args[0], "sum", err)?;
    let mut total = 0.0;
    for i in 0..ns.len() {
        ev.budget.charge_op()?;
        total += cached_node_number::<D>(ev, ns.get(i))?;
    }
    number(total)
}

fn num1<'e, 'd, D: Dom<'d>, F>(
    ev: &mut Evaluation<'e, 'd, D>,
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

fn fn_floor<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, "floor", f64::floor)
}

fn fn_ceiling<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
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

fn fn_round<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
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

fn fn_css_class<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "nokogiri-builtin:css-class")?;
    two::<D, _>(ev, args, |hay, needle| {
        boolean(ws_token_match(Some(hay), Some(needle)))
    })
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
fn fn_local_name_is<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(args.len(), 1, 1, err.clone(), "nokogiri-builtin:local-name-is")?;
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
fn same_type<'e, 'd, D: Dom<'d>>(a: D::Node, b: D::Node, doc: D) -> bool {
    doc.local_name(a) == doc.local_name(b) && doc.ns_uri(a) == doc.ns_uri(b)
}

/// The 1-based position of `node` among its same-type element siblings: forward
/// counts the preceding siblings, otherwise the following ones (from the end).
fn of_type_pos<'e, 'd, D: Dom<'d>>(node: Option<D::Node>, forward: bool, doc: D) -> f64 {
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

fn fn_of_type_pos<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, true, doc))
}

fn fn_of_type_pos_last<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, false, doc))
}

pub use crate::falloc::cstr::str_alloc;
