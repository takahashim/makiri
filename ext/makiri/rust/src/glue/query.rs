//! `Node#xpath` / `#at_xpath` for both representations, and the path every
//! query takes from its arguments to a Ruby value.
//!
//!   `node.xpath(expr, [namespaces], [handler], namespace_matching: :strict)`
//!
//! One argument list for HTML and XML alike, read by [`QueryArgs::scan`]: a Hash
//! among the optional arguments binds namespace prefixes for this query, any
//! other non-nil one is the handler that answers unknown functions, and
//! `namespace_matching:` picks the mode. Any other keyword is a prefix binding
//! too, which is how Nokogiri's `xpath("//s:p", s: uri)` reads.
//!
//! `Makiri::XPathContext` - a TypedData holding a context and an AST cache - and
//! the handler bridge (`rb_protect`, raw `VALUE`s, `extern "C"`) touch the raw
//! Ruby ABI, so they live in [`crate::bridge::xpath`]; this module is the Ruby
//! surface built on them.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RArray, RHash, RModule, RString, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::bridge::string::ruby_try_verified_text_pair;
use crate::bridge::wrapper::keepalive_document;
use crate::bridge::xpath::{
    context_for, evaluate_query, parse_query, query_result, Answer, Cx, XPathCtx,
};
use crate::init::{MOD_HTML_NODE_METHODS, MOD_XML_NODE_METHODS};
use crate::xpath::ast::Ast;

/// A query's arguments, read once for every entry point.
pub struct QueryArgs {
    /// The expression or selector.
    pub text: Value,
    /// Prefix bindings for this query alone, if any were given.
    pub namespaces: Option<RHash>,
    /// Answers unknown functions; nil for none.
    pub handler: Value,
    /// `namespace_matching: :lax`.
    pub lax: bool,
}

impl QueryArgs {
    /// `(text, [namespaces], [handler], namespace_matching:, **prefix_bindings)`.
    ///
    /// The one-argument call - `node.xpath(expr)`, much the commonest - is
    /// answered before `scan_args` runs at all. That is not a
    /// micro-optimisation: `scan_args` with a keyword type allocates an empty
    /// Hash even when no keywords were passed, and `at_xpath` spends about 650ns
    /// per call in total, so the allocation and the symbol lookups behind it
    /// measured ~32% of it.
    pub fn scan(ruby: &Ruby, args: &[Value]) -> Result<QueryArgs, Error> {
        let nil = ruby.qnil().as_value();
        if let [text] = args {
            return Ok(QueryArgs {
                text: *text,
                namespaces: None,
                handler: nil,
                lax: false,
            });
        }
        let a = magnus::scan_args::scan_args::<
            (Value,),
            (Option<Value>, Option<Value>),
            (),
            (),
            RHash,
            (),
        >(args)?;
        let kw = Keywords::scan(ruby, a.keywords)?;
        let mut q = QueryArgs {
            text: a.required.0,
            namespaces: None,
            handler: nil,
            lax: kw.lax,
        };
        for v in [a.optional.0, a.optional.1].into_iter().flatten() {
            if v.is_nil() {
                continue;
            }
            match RHash::from_value(v) {
                Some(h) if q.namespaces.is_none() => q.namespaces = Some(h),
                None if q.handler.is_nil() => q.handler = v,
                _ => {
                    return Err(Error::new(
                        ruby.exception_arg_error(),
                        "expected at most one namespace Hash and one handler",
                    ))
                }
            }
        }
        /* Keywords other than `namespace_matching:` are prefix bindings, and
         * win over a Hash given positionally. */
        if let Some(rest) = kw.bindings {
            q.namespaces = Some(match q.namespaces {
                Some(h) => h.funcall("merge", (rest,))?,
                None => rest,
            });
        }
        Ok(q)
    }
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
    pub fn scan(ruby: &Ruby, keywords: RHash) -> Result<Keywords, Error> {
        if keywords.is_empty() {
            return Ok(Keywords {
                lax: false,
                bindings: None,
            });
        }
        let mode = ruby.sym_new("namespace_matching");
        let lax = match keywords.get(mode) {
            None => false,
            Some(v) => matching_lax(ruby, v)?,
        };
        /* The rest are prefix bindings - how Nokogiri's `xpath("//s:p", s: uri)`
         * reads. Copied rather than mutated: the caller's Hash is its own. */
        let bindings: RHash = keywords.funcall("dup", ())?;
        let _: Value = bindings.funcall("delete", (mode,))?;
        Ok(Keywords {
            lax,
            bindings: (!bindings.is_empty()).then_some(bindings),
        })
    }
}

/// `namespace_matching:`'s value as the unprefixed-lax flag.
///
/// `:strict` (the default) resolves an unprefixed name test in the HTML
/// namespace, which is what browsers do; `:lax` makes it namespace-agnostic.
fn matching_lax(ruby: &Ruby, v: Value) -> Result<bool, Error> {
    if v.is_nil() || v.eql(ruby.sym_new("strict"))? {
        return Ok(false);
    }
    if v.eql(ruby.sym_new("lax"))? {
        return Ok(true);
    }
    Err(Error::new(
        ruby.exception_arg_error(),
        format!(
            "namespace_matching: must be :strict or :lax, got {}",
            v.inspect()
        ),
    ))
}

/// Bind every `{prefix => uri}` pair of `h` through `register`, which is what
/// differs between a per-query context and an `XPathContext`.
///
/// On any bad entry the error is returned and nothing further is registered -
/// the caller's owner then frees the context, so a partial registration is
/// never handed out.
fn bind_each(
    h: RHash,
    mut register: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
    cap: usize,
) -> Result<(), Error> {
    let pairs: RArray = h.funcall("to_a", ())?;
    for pair in pairs.into_iter() {
        let pair = RArray::from_value(pair).expect("Hash#to_a yields pairs");
        bind_pair(pair.entry(0)?, pair.entry(1)?, cap, &mut register)?;
    }
    Ok(())
}

/// Bind one `prefix => uri` pair through `register` - the one reading of a
/// namespace binding, for a query's Hash, `XPathContext.new`'s and
/// `#register_namespace` alike, so all three convert, check, cap and word a
/// refusal the same way.
///
/// Both are converted with `to_s` FIRST, and only then checked: a conversion
/// is Ruby code, and a view of the first held across the second's `to_s` is
/// what let that code rewrite a checked prefix.
pub fn bind_pair(
    prefix: Value,
    uri: Value,
    cap: usize,
    mut register: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let ks: RString = prefix.funcall("to_s", ())?;
    let vs: RString = uri.funcall("to_s", ())?;
    /* Both are the Strings `to_s` just returned, and the checks allocate
     * nothing, so the views stay valid through the registration below. */
    let (pv, uv) =
        ruby_try_verified_text_pair(ks.as_value(), vs.as_value(), cap).map_err(|reason| {
            makiri_error(format!(
                "invalid namespace mapping: {}",
                reason.to_string_lossy()
            ))
        })?;
    register(pv.as_verified().as_bytes(), uv.as_verified().as_bytes())
}

/// Register a `{prefix => uri}` Hash onto `ctx` for one query.
///
/// RSS and Atom live in a default namespace, so a prefix is the strict-mode way
/// to select them.
pub fn register_namespaces(ctx: &Cx, namespaces: Option<RHash>) -> Result<(), Error> {
    let Some(h) = namespaces else {
        return Ok(());
    };
    let cap = ctx.limits().max_string_bytes;
    bind_each(
        h,
        |prefix, uri| {
            ctx.register_ns(prefix, uri)
                .map_err(|_| makiri_error("failed to register namespace"))
        },
        cap,
    )
}

/// Register a `{prefix => uri}` Hash onto an `XPathContext`, for every later
/// evaluate.
pub fn register_bindings(ctx: &XPathCtx, bindings: RHash) -> Result<(), Error> {
    let cap = ctx.limits().max_string_bytes;
    bind_each(bindings, |prefix, uri| ctx.bind_namespace(prefix, uri), cap)
}

/// The context a query on `rb_self` runs under: rooted there (a Document's is
/// its document node), in the chosen mode, with the query's own prefixes.
pub fn query_context(rb_self: Value, document: Value, q: &QueryArgs) -> Result<Cx, Error> {
    let mut ctx = context_for(rb_self, document)?;
    ctx.set_lax(q.lax);
    register_namespaces(&ctx, q.namespaces)?; /* ctx drops on error */
    Ok(ctx)
}

/// Evaluate `ast` under `ctx` and convert the value for Ruby.
///
/// Both are taken by value and dropped BEFORE the conversion, which allocates
/// Ruby objects and so may raise or collect: the value owns its data and
/// references neither, so nothing is held that a raise would leak.
pub fn run_query(
    ctx: Cx,
    ast: Box<Ast>,
    handler: Value,
    document: Value,
    answer: Answer,
) -> Result<Value, Error> {
    let value = evaluate_query(&ctx, &ast, handler, document, answer);
    drop(ast);
    drop(ctx);
    query_result(value?, document, answer)
}

/// A throwaway context per call, so `Node#xpath` caches nothing;
/// `Makiri::XPathContext` is what a caller reaches for when many queries share
/// one namespace set and one set of compiled expressions.
fn xpath_run(rb_self: Value, q: QueryArgs, answer: Answer) -> Result<Value, Error> {
    let document = keepalive_document(rb_self)?;
    let ctx = query_context(rb_self, document, &q)?;
    /* Parsed AFTER the namespaces are registered: that step runs Ruby and may
     * collect, and the borrowed expression bytes must not be live across it. */
    let ast = parse_query(&ctx, q.text)?;
    run_query(ctx, ast, q.handler, document, answer)
}

fn node_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| xpath_run(rb_self, QueryArgs::scan(ruby, args)?, Answer::All))
}

/// The first matching node for a node-set result, or the scalar otherwise.
fn node_at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| xpath_run(rb_self, QueryArgs::scan(ruby, args)?, Answer::First))
}

/// `#xpath` / `#at_xpath` on both node-method modules. From `Init_makiri`,
/// after the classes exist.
pub fn init_xpath() {
    for module in [&MOD_HTML_NODE_METHODS, &MOD_XML_NODE_METHODS] {
        let m = RModule::from_value(module.value()).expect("a NodeMethods module");
        m.define_method("xpath", method!(node_xpath, -1))
            .expect("#xpath");
        m.define_method("at_xpath", method!(node_at_xpath, -1))
            .expect("#at_xpath");
    }
}
