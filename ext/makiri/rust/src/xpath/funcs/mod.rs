//! The built-in XPath 1.0 function library, plus the
//! two Nokogiri-compatible builtins and the CSS lowering's internal hooks.
//!
//! One function per builtin behind one signature, and [`BUILTINS`] is the only
//! place that names them - with their purity beside each. Keeping "does it exist" and "what does it do" as one
//! question matters here: the evaluator asks before deciding whether to route a
//! call to a Ruby handler, so a second list of names maintained separately could
//! disagree with this one and turn an unknown function into a bare failure.
//!
//! Where HTML and XML differ (`id()`, `lang()`), the function asks the host's
//! policy item on `Dom` rather than which host it is.

#![forbid(unsafe_code)]

/// The functions beyond XPath 1.0's library: the Nokogiri builtins and the
/// CSS lowering's internal hooks.
mod ext;
pub(crate) use ext::SiblingPositions;

use super::abi::*;
use super::axis::walk_descendants;
use super::dom::*;
use super::eval::Evaluation;
use super::order::nodeset_unique_sorted;
use super::value::Focus;
use super::value::*;
use crate::err_setf;
use crate::falloc::Reserve;
use core::ops::ControlFlow;

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
/// The same pair for the `-child` family: the position among ALL element
/// siblings. XPath can say that (`count(preceding-sibling::*) + 1`), but per
/// candidate it is n^2 over a flat list; the hook reads a per-parent memo.
pub const FN_CHILD_POS: &[u8] = b"\x01child-pos";
pub const FN_CHILD_POS_LAST: &[u8] = b"\x01child-pos-last";

/// Namespace URI registered from Nokogiri's XPath context, so prefixed names
/// like "nokogiri-builtin:css-class" resolve.
pub const NS_NOKOGIRI_BUILTIN_URI: &[u8] = b"https://www.nokogiri.org/default_ns/ruby/builtins";

/// A builtin step: the value, or proof its error was written to the context's
/// budget.
pub type FnResult<T = ()> = Result<T, Reported>;

/// What a builtin returns: its result, owned, so a caller that fails after
/// receiving it still clears it.
pub type Answer<N> = FnResult<Val<N>>;

/// Every built-in has this shape. The engine owns
/// `args` and clears them after the call.
pub type FnImpl<'e, 'd, D> = fn(
    &mut Evaluation<'e, 'd, D>,
    &Focus<'d, D>,
    &[Val<<D as Dom<'d>>::Node>],
) -> Answer<<D as Dom<'d>>::Node>;

/// Which library a built-in belongs to: the namespace its name is looked up in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Library {
    /// XPath 1.0's own, unprefixed - plus the CSS lowering's hooks, whose
    /// names no expression can spell.
    Core,
    /// Nokogiri's builtins, under [`NS_NOKOGIRI_BUILTIN_URI`].
    Nokogiri,
}

/// Whether a call may be evaluated once per evaluate and its value reused -
/// the hoisting pass in `ast_ops` asks. Stated beside each name in
/// [`BUILTINS`], so a function added there says it at the same time, and an
/// unknown name is never taken for pure.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purity {
    /// Depends on its arguments alone, even with none (`true()`).
    Pure,
    /// Depends on its arguments alone when it has any; with none it reads the
    /// context node (`string-length()`, `number()`).
    PureWithArgs,
    /// Reads the context node, position or size, or dynamic state - never
    /// reused. Deliberately also the class of pure functions the pass has not
    /// been taught, which keeps it conservative.
    Impure,
}

impl Purity {
    /// Whether a call with `nargs` arguments may be reused.
    pub fn allows(self, nargs: usize) -> bool {
        match self {
            Purity::Pure => true,
            Purity::PureWithArgs => nargs > 0,
            Purity::Impure => false,
        }
    }
}

/// Every built-in, as a name-free id: [`Builtin::imp`] maps each to its
/// function with a `match` the compiler checks is complete.
#[derive(Clone, Copy)]
enum Builtin {
    Last,
    Position,
    Count,
    Id,
    LocalName,
    NamespaceUri,
    Name,
    String,
    Concat,
    StartsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    Not,
    True,
    False,
    Boolean,
    Lang,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
    CssClass,
    LocalNameIs,
    OfTypePos,
    OfTypePosLast,
    ChildPos,
    ChildPosLast,
}

/// One [`BUILTINS`] row.
struct Entry {
    library: Library,
    name: &'static [u8],
    id: Builtin,
    purity: Purity,
    /// The least and most arguments it takes; `usize::MAX` is no upper bound.
    min: usize,
    max: usize,
}

/// A row, positionally, so the table stays one line per built-in.
const fn e(
    library: Library,
    name: &'static [u8],
    id: Builtin,
    purity: Purity,
    min: usize,
    max: usize,
) -> Entry {
    Entry {
        library,
        name,
        id,
        purity,
        min,
        max,
    }
}

/// THE list of built-ins: each name once, with its library, its purity and how
/// many arguments it takes (`min`, `max`). The evaluator asks it whether a call
/// is a built-in before routing it to a Ruby handler, and checks the count
/// against it before the call; the hoisting pass asks it whether a call is pure.
/// So there is no second list of names, or of arities, to fall out of step
/// with this one.
#[rustfmt::skip]
const BUILTINS: &[Entry] = {
    use Builtin::*;
    use Library::*;
    use Purity::*;
    &[
        /* node-set */
        e(Core, b"last", Last, Impure, 0, 0),
        e(Core, b"position", Position, Impure, 0, 0),
        e(Core, b"count", Count, PureWithArgs, 1, 1),
        e(Core, b"id", Id, Impure, 1, 1),
        e(Core, b"local-name", LocalName, Impure, 0, 1),
        e(Core, b"namespace-uri", NamespaceUri, Impure, 0, 1),
        e(Core, b"name", Name, Impure, 0, 1),
        /* string */
        e(Core, b"string", String, Impure, 0, 1),
        e(Core, b"concat", Concat, PureWithArgs, 2, usize::MAX),
        e(Core, b"starts-with", StartsWith, PureWithArgs, 2, 2),
        e(Core, b"contains", Contains, PureWithArgs, 2, 2),
        e(Core, b"substring-before", SubstringBefore, PureWithArgs, 2, 2),
        e(Core, b"substring-after", SubstringAfter, PureWithArgs, 2, 2),
        e(Core, b"substring", Substring, PureWithArgs, 2, 3),
        e(Core, b"string-length", StringLength, PureWithArgs, 0, 1),
        e(Core, b"normalize-space", NormalizeSpace, Impure, 0, 1),
        e(Core, b"translate", Translate, PureWithArgs, 3, 3),
        /* boolean */
        e(Core, b"not", Not, PureWithArgs, 1, 1),
        e(Core, b"true", True, Pure, 0, 0),
        e(Core, b"false", False, Pure, 0, 0),
        e(Core, b"boolean", Boolean, PureWithArgs, 1, 1),
        e(Core, b"lang", Lang, Impure, 1, 1),
        /* number */
        e(Core, b"number", Number, PureWithArgs, 0, 1),
        e(Core, b"sum", Sum, PureWithArgs, 1, 1),
        e(Core, b"floor", Floor, PureWithArgs, 1, 1),
        e(Core, b"ceiling", Ceiling, PureWithArgs, 1, 1),
        e(Core, b"round", Round, PureWithArgs, 1, 1),
        /* The CSS lowering's internal hooks. Registered for every host: their
         * names begin with \x01, which no expression can spell, so only the
         * lowering reaches them - and it runs only for XML today. */
        e(Core, FN_OF_TYPE_POS, OfTypePos, Impure, 0, usize::MAX),
        e(Core, FN_OF_TYPE_POS_LAST, OfTypePosLast, Impure, 0, usize::MAX),
        e(Core, FN_CHILD_POS, ChildPos, Impure, 0, usize::MAX),
        e(Core, FN_CHILD_POS_LAST, ChildPosLast, Impure, 0, usize::MAX),
        /* Nokogiri's builtins, in its builtin namespace */
        e(Nokogiri, b"css-class", CssClass, Impure, 2, 2),
        e(Nokogiri, b"local-name-is", LocalNameIs, Impure, 1, 1),
    ]
};

impl Builtin {
    fn imp<'e, 'd, D: Dom<'d>>(self) -> FnImpl<'e, 'd, D> {
        match self {
            Builtin::Last => fn_last::<D> as FnImpl<'e, 'd, D>,
            Builtin::Position => fn_position::<D> as FnImpl<'e, 'd, D>,
            Builtin::Count => fn_count::<D> as FnImpl<'e, 'd, D>,
            Builtin::Id => fn_id::<D> as FnImpl<'e, 'd, D>,
            Builtin::LocalName => fn_local_name::<D> as FnImpl<'e, 'd, D>,
            Builtin::NamespaceUri => fn_namespace_uri::<D> as FnImpl<'e, 'd, D>,
            Builtin::Name => fn_name::<D> as FnImpl<'e, 'd, D>,
            Builtin::String => fn_string::<D> as FnImpl<'e, 'd, D>,
            Builtin::Concat => fn_concat::<D> as FnImpl<'e, 'd, D>,
            Builtin::StartsWith => fn_starts_with::<D> as FnImpl<'e, 'd, D>,
            Builtin::Contains => fn_contains::<D> as FnImpl<'e, 'd, D>,
            Builtin::SubstringBefore => fn_substring_before::<D> as FnImpl<'e, 'd, D>,
            Builtin::SubstringAfter => fn_substring_after::<D> as FnImpl<'e, 'd, D>,
            Builtin::Substring => fn_substring::<D> as FnImpl<'e, 'd, D>,
            Builtin::StringLength => fn_string_length::<D> as FnImpl<'e, 'd, D>,
            Builtin::NormalizeSpace => fn_normalize_space::<D> as FnImpl<'e, 'd, D>,
            Builtin::Translate => fn_translate::<D> as FnImpl<'e, 'd, D>,
            Builtin::Not => fn_not::<D> as FnImpl<'e, 'd, D>,
            Builtin::True => fn_true::<D> as FnImpl<'e, 'd, D>,
            Builtin::False => fn_false::<D> as FnImpl<'e, 'd, D>,
            Builtin::Boolean => fn_boolean::<D> as FnImpl<'e, 'd, D>,
            Builtin::Lang => fn_lang::<D> as FnImpl<'e, 'd, D>,
            Builtin::Number => fn_number::<D> as FnImpl<'e, 'd, D>,
            Builtin::Sum => fn_sum::<D> as FnImpl<'e, 'd, D>,
            Builtin::Floor => fn_floor::<D> as FnImpl<'e, 'd, D>,
            Builtin::Ceiling => fn_ceiling::<D> as FnImpl<'e, 'd, D>,
            Builtin::Round => fn_round::<D> as FnImpl<'e, 'd, D>,
            Builtin::CssClass => ext::fn_css_class::<D> as FnImpl<'e, 'd, D>,
            Builtin::LocalNameIs => ext::fn_local_name_is::<D> as FnImpl<'e, 'd, D>,
            Builtin::OfTypePos => ext::fn_of_type_pos::<D> as FnImpl<'e, 'd, D>,
            Builtin::OfTypePosLast => ext::fn_of_type_pos_last::<D> as FnImpl<'e, 'd, D>,
            Builtin::ChildPos => ext::fn_child_pos::<D> as FnImpl<'e, 'd, D>,
            Builtin::ChildPosLast => ext::fn_child_pos_last::<D> as FnImpl<'e, 'd, D>,
        }
    }
}

/// The table entry for `local` in `library`.
fn find(library: Library, local: &[u8]) -> Option<&'static Entry> {
    BUILTINS
        .iter()
        .find(|e| e.library == library && e.name == local)
}

/// The built-in named `(ns_uri, local)`, or None - in which case the evaluator
/// routes the call to the registered resolver. The Nokogiri builtins live in
/// one namespace; any other registered namespace means a user-defined function.
pub fn lookup<'e, 'd, D: Dom<'d>>(ns_uri: Option<&[u8]>, local: &[u8]) -> Option<Found<'e, 'd, D>> {
    let library = match ns_uri {
        None => Library::Core,
        Some(uri) if uri == NS_NOKOGIRI_BUILTIN_URI => Library::Nokogiri,
        Some(_) => return None,
    };
    find(library, local).map(|e| Found {
        imp: e.id.imp::<D>(),
        library: e.library,
        name: e.name,
        min: e.min,
        max: e.max,
    })
}

/// A built-in [`lookup`] found: the function, and what [`Found::call`] checks
/// before running it.
pub struct Found<'e, 'd, D: Dom<'d>> {
    imp: FnImpl<'e, 'd, D>,
    library: Library,
    name: &'static [u8],
    min: usize,
    max: usize,
}

impl<'e, 'd, D: Dom<'d>> Found<'e, 'd, D> {
    /// Check the argument count against the table, then call.
    pub fn call(
        &self,
        ev: &mut Evaluation<'e, 'd, D>,
        focus: &Focus<'d, D>,
        args: &[Val<D::Node>],
    ) -> Answer<D::Node> {
        let got = args.len();
        if got < self.min || got > self.max {
            return Err(self.arity_error(ev.budget.sink(), got));
        }
        (self.imp)(ev, focus, args)
    }

    #[cold]
    fn arity_error(&self, err: ErrSink, got: usize) -> Reported {
        let (min, max) = (self.min, self.max);
        let lib = match self.library {
            Library::Core => "",
            Library::Nokogiri => "nokogiri-builtin:",
        };
        let name = crate::engine_error::Bytes(self.name);
        if max == usize::MAX {
            err_setf!(
                err,
                ErrorKind::Runtime,
                "{}{}(): expected at least {} argument{}",
                lib,
                name,
                min,
                if min == 1 { "" } else { "s" }
            )
        } else if min == max {
            err_setf!(
                err,
                ErrorKind::Runtime,
                "{}{}(): expected {} argument(s), got {}",
                lib,
                name,
                min,
                got
            )
        } else {
            err_setf!(
                err,
                ErrorKind::Runtime,
                "{}{}(): expected {}-{} argument(s), got {}",
                lib,
                name,
                min,
                max,
                got
            )
        }
    }
}

/// The purity of the unprefixed call `local`; [`Purity::Impure`] for a name
/// that is not a core built-in, which a handler answers.
pub fn purity(local: &[u8]) -> Purity {
    find(Library::Core, local).map_or(Purity::Impure, |e| e.purity)
}

/* ---------- shared helpers ---------- */

/// The shared "argument must be a node-set" check.
fn require_nodeset<'v, N>(arg: &'v Val<N>, fname: &str, err: ErrSink) -> FnResult<&'v NodeSet<N>> {
    match arg.as_nodeset() {
        Some(ns) => Ok(ns),
        None => Err(err_setf!(
            err,
            ErrorKind::Type,
            "{}(): argument must be a node-set",
            fname
        )),
    }
}

/// An owned copy of `s`, or `Err` with `*err` naming `what` on OOM.
fn c_string(s: &[u8], err: ErrSink, what: &str) -> FnResult<Text> {
    Text::try_copy(s).ok_or_else(|| err_setf!(err, ErrorKind::Oom, "out of memory in {}()", what))
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

fn to_number<'e, 'd, D: Dom<'d>>(
    v: &Val<D::Node>,
    ev: &mut Evaluation<'e, 'd, D>,
) -> FnResult<f64> {
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
fn self_text<'e, 'd, D: Dom<'d>>(
    focus: &Focus<'d, D>,
    ev: &mut Evaluation<'e, 'd, D>,
) -> FnResult<Text> {
    match focus.node {
        Some(n) => node_to_owned_text::<D>(ev.doc, n, &mut ev.budget),
        None => owned_copy(
            b"",
            ev.budget.sink(),
            "out of memory building node string-value",
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

/// The byte offset of the first `needle` in `hay`, in time linear in both and
/// with no allocation.
///
/// A `windows(n).position(..)` scan is O(hay x needle), which the input picks:
/// `contains()` of a 400 KB string took 2.4 s. `str::find` runs std's Two-Way
/// search, which is linear; XPath strings are valid UTF-8 (DOM text, verified
/// expressions and variables), and a match of valid UTF-8 in valid UTF-8 starts
/// on a character boundary, so its offset is the byte search's. Called once
/// per function call - it validates both strings, so a caller must not loop
/// it over the rest of one haystack. Bytes that are not UTF-8 get the plain
/// scan: slower, never wrong.
fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    match (core::str::from_utf8(hay), core::str::from_utf8(needle)) {
        (Ok(h), Ok(n)) => h.find(n),
        _ if needle.is_empty() => Some(0),
        _ => hay.windows(needle.len()).position(|w| w == needle),
    }
}

/// The number of characters in valid UTF-8: every byte that is not a
/// continuation byte starts one.
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
/// the abort a plain `Vec` growth gives (std's allocation failure aborts, it
/// does not unwind).
fn try_vec<T>(n: usize, err: ErrSink, what: &str) -> FnResult<Vec<T>> {
    let mut v: Vec<T> = Vec::new();
    if v.falloc_reserve_exact(n).is_err() {
        return Err(err_setf!(
            err,
            ErrorKind::Oom,
            "out of memory in {}()",
            what
        ));
    }
    Ok(v)
}

/* ---------- node-set functions ---------- */

fn fn_last<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(focus.size as f64)
}

fn fn_position<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(focus.pos as f64)
}

fn fn_count<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let ns = require_nodeset(&args[0], "count", err)?;
    number(ns.len() as f64)
}

/// Walk the tree for an element whose `id_attr` attribute is `id`.
///
/// Every visited node is charged to the op budget: without it, id() over a large
/// node-set - a token per node, a tree walk per token - drives quadratic work at
/// no cost. Returns Err on an overrun, with the budget's slot set.
fn find_by_id<'e, 'd, D: Dom<'d>>(
    doc: D,
    root: D::Node,
    id_attr: &[u8],
    id: &[u8],
    budget: &mut Budget,
) -> Result<Option<D::Node>, Reported> {
    if id.is_empty() {
        return Ok(None);
    }
    /* `root` is the document node, never an element, so its descendants are
     * every element there is. */
    let flow = walk_descendants::<D, _, _>(doc, root, &mut |n| {
        if let Err(e) = budget.charge_op() {
            return ControlFlow::Break(Err(e));
        }
        if doc.node_type(n) == NodeType::Element && doc.get_attribute(n, id_attr) == Some(id) {
            return ControlFlow::Break(Ok(n));
        }
        ControlFlow::Continue(())
    });
    match flow {
        ControlFlow::Continue(()) => Ok(None),
        ControlFlow::Break(found) => found.map(Some),
    }
}

/// Look up every whitespace-separated token of `s` and push each hit.
///
/// Duplicates go in unconditionally: the caller dedups the whole result with one
/// sort plus an adjacent pass, which beats a contains() check per insert.
fn id_collect<'e, 'd, D: Dom<'d>>(
    id_attr: &[u8],
    s: &[u8],
    root: D::Node,
    out: &mut NodeSet<D::Node>,
    ev: &mut Evaluation<'e, 'd, D>,
) -> FnResult {
    let doc = ev.doc;
    let budget = &mut ev.budget;
    for tok in s
        .split(|&b| crate::xpath::lex::is_ws(b))
        .filter(|t| !t.is_empty())
    {
        if let Some(hit) = find_by_id::<D>(doc, root, id_attr, tok, budget)? {
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
    /* A host with no ID attributes answers the empty node-set - see
     * `Dom::ID_ATTRIBUTE`. */
    let Some(id_attr) = D::ID_ATTRIBUTE else {
        return Ok(Val::default());
    };
    let doc = ev.doc;
    let root = doc.document_node();
    /* Collected in a guard, so a failure part-way frees what was found. */
    let mut found = NodeSet::new();

    /* §4.1: a node-set argument treats each node's string-value as IDREFS;
     * anything else is converted to a string and split the same way. */
    if let Some(set) = args[0].as_nodeset() {
        set.as_slice().iter().try_for_each(|&n| {
            let t = node_to_owned_text::<D>(doc, n, &mut ev.budget)?;
            id_collect::<D>(id_attr, t.as_slice(), root, &mut found, ev)
        })?;
    } else {
        let t = to_text::<D>(&args[0], ev)?;
        id_collect::<D>(id_attr, t.as_slice(), root, &mut found, ev)?;
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
    Ok(ns.as_slice().first().copied())
}

/// `n`'s local or qualified name as a string result; anything that is not an
/// element, attribute or PI yields "". A PI's name is its target either way (its
/// expanded-name is (null, target)). In HTML the qualified name equals the local
/// name, which also keeps the LXB_NS_HTML prefix out of the result.
/// Which name `local-name()` / `name()` reads. The function's own name - for
/// its error messages - comes with it, so the two cannot disagree.
#[derive(Clone, Copy)]
enum NameKind {
    Local,
    Qualified,
}

impl NameKind {
    fn fname(self) -> &'static str {
        match self {
            NameKind::Local => "local-name",
            NameKind::Qualified => "name",
        }
    }
}

fn name_emit<'e, 'd, D: Dom<'d>>(
    doc: D,
    n: Option<D::Node>,
    kind: NameKind,
    err: ErrSink,
) -> Answer<D::Node> {
    let fname = kind.fname();
    let Some(n) = n else {
        return string(b"", err, fname);
    };
    let name: &[u8] = if let Some(a) = doc.as_attr(n) {
        match kind {
            NameKind::Local => doc.attr_local_name(a),
            NameKind::Qualified => doc.attr_qualified_name(a),
        }
    } else {
        match (doc.node_type(n), kind) {
            (NodeType::Element, NameKind::Local) => doc.local_name(n),
            (NodeType::Element, NameKind::Qualified) => doc.qualified_name(n),
            (NodeType::Pi, _) => doc.pi_name(n),
            _ => b"",
        }
    };
    string(name, err, fname)
}

/// `local-name()` and `name()`: the target node's local or qualified name.
fn name_of<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
    kind: NameKind,
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let t = name_target::<D>(args, focus, err.clone(), kind.fname())?;
    name_emit::<D>(ev.doc, t, kind, err)
}

fn fn_local_name<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    name_of(ev, focus, args, NameKind::Local)
}

fn fn_name<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    name_of(ev, focus, args, NameKind::Qualified)
}

fn fn_namespace_uri<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    let Some(t) = name_target::<D>(args, focus, err.clone(), "namespace-uri")? else {
        return string(b"", err.clone(), "namespace-uri");
    };
    /* An attribute's own namespace, never its element's - see
     * `Dom::attr_ns_uri`. */
    let uri = match doc.as_attr(t) {
        Some(a) => doc.attr_ns_uri(a),
        None if doc.node_type(t) == NodeType::Element && doc.has_ns(t) => doc.ns_uri(t),
        None => b"",
    };
    string(uri, err.clone(), "namespace-uri")
}

/* ---------- string functions ---------- */

fn fn_string<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    Ok(Val::string(arg_or_self_text::<D>(focus, args, ev)?))
}

fn fn_concat<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let mut parts = try_vec::<Text>(args.len(), err.clone(), "concat")?;
    let mut total = 0usize;
    for a in args {
        let t = to_text::<D>(a, ev)?;
        total = match total.checked_add(t.as_slice().len()) {
            Some(n) => n,
            None => return Err(err_setf!(err, ErrorKind::Oom, "concat() size overflow")),
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
        return Err(err_setf!(err, ErrorKind::Oom, "out of memory in concat()"));
    };
    Ok(Val::string(joined))
}

fn fn_starts_with<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    two::<D, _>(ev, args, |s, t| boolean(s.starts_with(t)))
}

fn fn_contains<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    two::<D, _>(ev, args, |s, t| boolean(find_bytes(s, t).is_some()))
}

fn fn_substring_before<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
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
    let s = to_text::<D>(&args[0], ev)?;
    let bytes = s.as_slice();
    let nchars = count_chars(bytes);
    /* §4.2: the characters at positions p with
     *   round(start) <= p < round(start) + round(length)
     * - each argument rounded on its own, by round()'s own rule. Rounding the
     * SUM instead gave substring("12345", 1.5, 2.6) = "23" where the spec's own
     * example is "234", and floor(x + 0.5) rounded 0.49999999999999994 up. */
    let rstart = round_half_up(to_number::<D>(&args[1], ev)?);
    let end_d = match args.get(2) {
        Some(a) => rstart + round_half_up(to_number::<D>(a, ev)?),
        None => f64::INFINITY,
    };

    if rstart.is_nan() || end_d.is_nan() {
        return string(b"", err.clone(), "substring");
    }
    /* Clamp AS DOUBLES before any cast: start/end can be infinite or beyond
     * i64 (`substring(s, 1 div 0)`), where a cast would saturate - not the
     * spec's clip. */
    let imax = nchars as f64 + 1.0;
    let rstart = rstart.clamp(1.0, imax);
    let rend = end_d.clamp(1.0, imax);
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
    let s = arg_or_self_text::<D>(focus, args, ev)?;
    let src = s.as_slice();
    let normalized = Text::try_fill(src.len(), |dst| {
        let mut w = 0usize;
        let mut in_space = true;
        for &c in src {
            if crate::xpath::lex::is_ws(c) {
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
            ErrorKind::Oom,
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
    let [s, f, t] = args else {
        /* The dispatch table's arity check makes this unreachable: reaching it
         * is a broken invariant, not the caller's mistake. */
        return Err(err_setf!(
            err,
            ErrorKind::Internal,
            "translate() reached with {} arguments",
            args.len()
        ));
    };
    /* Converted left to right, as the arguments were written. */
    let (s, f, t) = (
        to_text::<D>(s, ev)?,
        to_text::<D>(f, ev)?,
        to_text::<D>(t, ev)?,
    );
    let (sv, fv, tv) = match (
        core::str::from_utf8(s.as_slice()),
        core::str::from_utf8(f.as_slice()),
        core::str::from_utf8(t.as_slice()),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => {
            return Err(err_setf!(
                err,
                ErrorKind::Runtime,
                "invalid UTF-8 in translate() argument"
            ));
        }
    };
    /* A character is never shorter than a byte, so the byte length bounds the
     * count - reserving up front keeps a failed allocation an XPath OOM rather
     * than the abort a growing Vec gives on allocation failure. */
    /* `from` as (character, its FIRST position), sorted by character, so each
     * input character is a binary search rather than a scan of `from`: that
     * scan was O(string x from), both up to the byte cap. */
    let mut from_cp = try_vec::<(char, usize)>(fv.len(), err.clone(), "translate")?;
    from_cp.extend(fv.chars().enumerate().map(|(k, c)| (c, k)));
    from_cp.sort_unstable();
    from_cp.dedup_by_key(|&mut (c, _)| c); /* keeps the first, the smallest k */
    let mut to_cp = try_vec::<char>(tv.len(), err.clone(), "translate")?;
    to_cp.extend(tv.chars());

    /* Capped: a multibyte replacement can push the result past the limit even
     * when the input is inside it ("a" -> an emoji), so the append fails closed
     * with LIMIT or OOM. */
    let mut buf = Buf::new(ev.budget.limits.max_string_bytes);
    let mut enc = [0u8; 4];
    for c in sv.chars() {
        let found = from_cp
            .binary_search_by_key(&c, |&(f, _)| f)
            .ok()
            .map(|i| from_cp[i].1);
        let emit: Option<&str> = match found {
            None => Some(c.encode_utf8(&mut enc)), /* not in `from`: keep it */
            Some(k) if k < to_cp.len() => Some(to_cp[k].encode_utf8(&mut enc)),
            Some(_) => None, /* past `to`: drop it */
        };
        if let Some(e) = emit {
            buf.append(e.as_bytes()).map_err(|e| match e {
                crate::cbuf::BufError::Limit => err_setf!(
                    err,
                    ErrorKind::Limit,
                    "string size limit exceeded ({} bytes) in translate()",
                    ev.budget.limits.max_string_bytes
                ),
                crate::cbuf::BufError::Oom => {
                    err_setf!(err, ErrorKind::Oom, "out of memory in translate()")
                }
            })?;
        }
    }
    let owned = buf
        .steal()
        .map_err(|_| err_setf!(err, ErrorKind::Oom, "out of memory in translate()"))?;
    Ok(Val::string(Text::from_buf(owned)))
}

/* ---------- boolean functions ---------- */

fn fn_not<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    boolean(!val_to_boolean(&args[0]))
}

fn fn_true<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    boolean(true)
}

fn fn_false<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    boolean(false)
}

fn fn_boolean<'e, 'd, D: Dom<'d>>(
    _ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    boolean(val_to_boolean(&args[0]))
}

fn fn_lang<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    let want = to_text::<D>(&args[0], ev)?;
    let want = want.as_slice();
    /* Walk the ancestors for the host's language attributes
     * (`Dom::LANG_ATTRIBUTES`), a tick each: a predicate runs this per
     * candidate, and `//span[lang('en')]` over 16,000 nested spans climbed a
     * quadratic number of ancestors, uncharged, for 4.5 s. */
    let mut p = focus.node;
    while let Some(n) = p {
        ev.budget.charge_op()?;
        if doc.node_type(n) == NodeType::Element {
            let v = D::LANG_ATTRIBUTES
                .iter()
                .find_map(|name| doc.get_attribute(n, name));
            if let Some(v) = v {
                /* The NEAREST element carrying the attribute decides (§4.3):
                 * a non-matching one ends the walk rather than letting an
                 * ancestor's answer through. Case-insensitive compare of the
                 * prefix up to a '-'. */
                return boolean(
                    v.len() >= want.len()
                        && v[..want.len()].eq_ignore_ascii_case(want)
                        && (v.len() == want.len() || v[want.len()] == b'-'),
                );
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
    let ns = require_nodeset(&args[0], "sum", err)?;
    let mut total = 0.0;
    for &n in ns.as_slice() {
        ev.budget.charge_op()?;
        total += cached_node_number::<D>(ev, n)?;
    }
    number(total)
}

fn num1<'e, 'd, D: Dom<'d>, F>(
    ev: &mut Evaluation<'e, 'd, D>,
    args: &[Val<D::Node>],
    f: F,
) -> Answer<D::Node>
where
    F: FnOnce(f64) -> f64,
{
    number(f(to_number::<D>(&args[0], ev)?))
}

fn fn_floor<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, f64::floor)
}

fn fn_ceiling<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    num1::<D, _>(ev, args, f64::ceil)
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
    num1::<D, _>(ev, args, round_half_up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_is_named_once_per_library() {
        for (i, a) in BUILTINS.iter().enumerate() {
            let twin = BUILTINS[i + 1..]
                .iter()
                .any(|b| a.library == b.library && a.name == b.name);
            assert!(!twin, "{} is listed twice", String::from_utf8_lossy(a.name));
        }
    }

    #[test]
    fn the_hoisting_pass_only_reuses_what_reads_no_context() {
        assert!(purity(b"true").allows(0));
        assert!(purity(b"concat").allows(2));
        /* With no argument these read the context node. */
        assert!(!purity(b"string-length").allows(0));
        assert!(!purity(b"number").allows(0));
        /* Context, position and dynamic state are never reused. */
        for name in [
            &b"last"[..],
            b"position",
            b"string",
            b"id",
            b"lang",
            b"local-name",
        ] {
            assert!(!purity(name).allows(1), "{}", String::from_utf8_lossy(name));
        }
        /* An unknown name is a handler's, and a handler is never taken for pure. */
        assert!(!purity(b"my-function").allows(1));
        /* A Nokogiri builtin is looked up under its namespace, not as core. */
        assert_eq!(purity(b"css-class"), Purity::Impure);
    }
}
