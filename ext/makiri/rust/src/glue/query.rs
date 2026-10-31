//! `Node#xpath` / `#at_xpath` for both representations, and the path every
//! query takes from its arguments to a Ruby value.
//!
//!   `node.xpath(expr, [namespaces], namespace_matching: :strict)`
//!
//! One argument list for HTML and XML alike, read by [`QueryArgs::scan`]: a Hash
//! binds namespace prefixes for this query, and `namespace_matching:` picks the
//! mode. Any other keyword is a prefix binding too, which is how Nokogiri's
//! `xpath("//s:p", s: uri)` reads. Nokogiri's custom-function handler - an
//! object whose methods answer unknown functions - is refused: Makiri runs no
//! Ruby while it evaluates, and an unknown function is an `XPath::SyntaxError`.
//!
//! `Makiri::XPathContext` - a TypedData holding a context and an AST cache -
//! touches the raw Ruby ABI, so it lives in [`crate::bridge::xpath`]; this
//! module is the Ruby surface built on it.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};
use crate::method;

use crate::bridge::args::Kwargs;
use crate::bridge::ruby::makiri_error;
use crate::bridge::wrapper::keepalive_document;
use crate::bridge::xpath::{
    context_for, evaluate_to_ruby, parse_query, Answer, Cx, QueryAst, XPathCtx,
};
use crate::init::{MOD_HTML_NODE_METHODS, MOD_XML_NODE_METHODS};

/// A query's arguments, read once for every entry point.
pub struct QueryArgs {
    /// The expression or selector.
    pub text: Value,
    /// Prefix bindings for this query alone, if any were given.
    pub namespaces: Option<RHash>,
    /// `namespace_matching: :lax`.
    pub lax: bool,
}

impl QueryArgs {
    /// `(text, [namespaces], namespace_matching:, **prefix_bindings)`.
    ///
    /// The one-argument call - `node.xpath(expr)`, much the commonest - is
    /// answered before the keywords are read at all. One argument that is
    /// the call's keywords (`xpath(s: uri)`, no expression) is not that call:
    /// it goes through the scan, whose arity error it is.
    pub fn scan(ruby: &Ruby, args: &[Value]) -> Result<QueryArgs, Error> {
        if let ([text], false) = (args, crate::bridge::rb::method::keyword_given(ruby)) {
            return Ok(QueryArgs {
                text: *text,
                namespaces: None,
                lax: false,
            });
        }
        let a =
            crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::with_keywords(1, 1))?;
        let kw = Keywords::scan(ruby, Kwargs::from_scan(a.keywords()))?;
        let mut q = QueryArgs {
            text: a.required(0),
            namespaces: None,
            lax: kw.lax,
        };
        if let Some(v) = a.optional(0) {
            match RHash::from_value(v) {
                Some(h) => q.namespaces = Some(h),
                None => return Err(Error::new(
                    crate::bridge::rb::Builtin::ArgumentError,
                    "expected a namespace Hash; custom XPath function handlers are not supported",
                )),
            }
        }
        /* Keywords other than `namespace_matching:` are prefix bindings, and
         * win over a Hash given positionally. */
        if let Some(rest) = kw.bindings {
            q.namespaces = Some(match q.namespaces {
                Some(h) => merge_hashes(ruby, h, rest)?,
                None => rest,
            });
        }
        Ok(q)
    }
}

/// `a.merge(b)` - a fresh plain Hash with `a`'s pairs then `b`'s (b wins), read
/// through the Hash storage rather than a Ruby `merge` a Hash subclass could
/// redefine. `rb_hash_foreach`/`rb_hash_aset` are the C operations, so no
/// caller's method runs.
fn merge_hashes(ruby: &Ruby, a: RHash, b: RHash) -> Result<RHash, Error> {
    let out: RHash = crate::bridge::build::hash(ruby)?;
    crate::bridge::hash::hash_foreach(a, |k, v| crate::bridge::build::aset(out, k, v))?;
    crate::bridge::hash::hash_foreach(b, |k, v| crate::bridge::build::aset(out, k, v))?;
    Ok(out)
}

/// What a query's keywords mean: the matching mode, and the prefix bindings
/// every other keyword makes.
///
/// One reading for both entry points that take them - `#xpath` and
/// `XPathContext.new` - so neither can quietly drop what the other binds.
pub struct Keywords {
    /// `namespace_matching: :lax`.
    pub lax: bool,
    /// Every other keyword, as `{prefix => uri}`; None when there is none.
    pub bindings: Option<RHash>,
}

impl Keywords {
    pub(crate) fn scan(ruby: &Ruby, keywords: Kwargs) -> Result<Keywords, Error> {
        let (mode, bindings) = keywords.split_off(ruby, "namespace_matching")?;
        let lax = match mode {
            None => false,
            Some(v) => matching_lax(ruby, v)?,
        };
        /* The rest are prefix bindings - how Nokogiri's `xpath("//s:p", s: uri)`
         * reads. */
        Ok(Keywords { lax, bindings })
    }
}

/// `namespace_matching:`'s value as the unprefixed-lax flag.
///
/// `:strict` (the default) resolves an unprefixed name test in the HTML
/// namespace, which is what browsers do; `:lax` makes it namespace-agnostic.
fn matching_lax(ruby: &Ruby, v: Value) -> Result<bool, Error> {
    /* By identity: a Symbol is unique, and `eql?` would be the caller's
     * value's method. */
    if v.is_nil()
        || crate::bridge::ruby::same_value(v, crate::bridge::build::symbol(ruby, "strict")?)
    {
        return Ok(false);
    }
    if crate::bridge::ruby::same_value(v, crate::bridge::build::symbol(ruby, "lax")?) {
        return Ok(true);
    }
    Err(Error::new(
        crate::bridge::rb::Builtin::ArgumentError,
        format_args!(
            "namespace_matching: must be :strict or :lax, got {}",
            crate::bridge::ruby::Describe(v)
        ),
    ))
}

/// Register a `{prefix => uri}` Hash onto `ctx` for one query.
///
/// RSS and Atom live in a default namespace, so a prefix is the strict-mode way
/// to select them.
pub fn register_namespaces(ruby: &Ruby, ctx: &Cx, namespaces: Option<RHash>) -> Result<(), Error> {
    let Some(h) = namespaces else {
        return Ok(());
    };
    let cap = ctx.limits().max_string_bytes;
    crate::glue::namespaces::bind_each(ruby, h, cap, |prefix, uri| {
        ctx.register_ns(prefix, uri)
            .map_err(|_| makiri_error("failed to register namespace"))
    })
}

/// Register a `{prefix => uri}` Hash onto an `XPathContext`, for every later
/// evaluate.
pub fn register_bindings(ruby: &Ruby, ctx: &XPathCtx, bindings: RHash) -> Result<(), Error> {
    let cap = ctx.limits().max_string_bytes;
    crate::glue::namespaces::bind_each(ruby, bindings, cap, |prefix, uri| {
        ctx.bind_namespace(prefix, uri)
    })
}

/// The context a query on `rb_self` runs under: rooted there (a Document's is
/// its document node), in the chosen mode, with the query's own prefixes.
pub fn query_context(
    ruby: &Ruby,
    rb_self: Value,
    document: Value,
    q: &QueryArgs,
) -> Result<Cx, Error> {
    let mut ctx = context_for(ruby, rb_self, document)?;
    ctx.set_lax(q.lax);
    register_namespaces(ruby, &ctx, q.namespaces)?; /* ctx drops on error */
    Ok(ctx)
}

/// A throwaway context per call, so `Node#xpath` caches nothing;
/// `Makiri::XPathContext` is what a caller reaches for when many queries share
/// one namespace set and one set of compiled expressions.
fn xpath_run(ruby: &Ruby, rb_self: Value, q: QueryArgs, answer: Answer) -> Result<Value, Error> {
    let document = keepalive_document(ruby, rb_self)?;
    let ctx = query_context(ruby, rb_self, document, &q)?;
    let ast = parse_query(ruby, &ctx, q.text)?;
    evaluate_to_ruby(ruby, ctx, QueryAst::owned(ast), document, answer)
}

fn node_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    xpath_run(ruby, rb_self, QueryArgs::scan(ruby, args)?, Answer::All)
}

/// The first matching node for a node-set result, or the scalar otherwise.
fn node_at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    xpath_run(ruby, rb_self, QueryArgs::scan(ruby, args)?, Answer::First)
}

/// `#xpath` / `#at_xpath` on both node-method modules. From `Init_makiri`,
/// after the classes exist.
pub fn init_xpath(ruby: &Ruby) -> Result<(), Error> {
    for module in [&MOD_HTML_NODE_METHODS, &MOD_XML_NODE_METHODS] {
        let m = module.defined(ruby)?;
        m.define_method("xpath", method!(node_xpath, -1))?;
        m.define_method("at_xpath", method!(node_at_xpath, -1))?;
    }
    Ok(())
}
