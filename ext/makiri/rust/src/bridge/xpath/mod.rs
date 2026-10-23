//! The Ruby <-> XPath engine seam: which backend a query runs on, building the
//! engine context for a Ruby node or document, and the safe dispatch the glue
//! calls.
//!
//! The engine (`xpath/`) is generic over `Dom` and names no representation, and
//! `glue/query.rs` must not name an HTML or XML type either. This module sits
//! between them: it is the one place that knows both backends exist, and it
//! turns a Ruby node into the right context with the raw-pointer work the
//! backend's lifetime contract needs.

#![allow(unsafe_code)]

use magnus::{Error, Value};

use crate::bridge::html::{html_node_key, with_html_node};
use crate::bridge::node_set::node_set_with_fill;
use crate::bridge::node_wrap::wrap_doc_node;
use crate::bridge::string::ruby_verified_text;
use crate::bridge::wrapper::{html_read, xml_read, DocKind, NodeWord, DOC_TYPE};
use crate::bridge::xml::xml_node_unwrap;
use crate::engine_error::{Error as XPathError, ErrorKind};
use crate::html_dom::xpath::HtmlDom;
use crate::init::EXC_ERROR;
pub use crate::init::{CLASS_XPATH_CONTEXT, EXC_XPATH_LIMIT_EXCEEDED, EXC_XPATH_SYNTAX_ERROR};
use crate::token::Token;
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Session, XPathValue};
use crate::xpath::dom::Dom;
use crate::xpath::limits::Budget;
use crate::xpath::value::ValRef;
use core::ptr::NonNull;

mod context_object;

pub use context_object::{XPathCtx, XPathCtxSelf};

/// An engine error as the Ruby exception it maps to.
///
/// Returned rather than raised: `rb_raise` longjmps past every Rust destructor
/// on the way (see `glue/mod.rs`), so each caller hands this back as `Err` and
/// magnus raises once its frames - and the context they own - are gone.
///
/// Built lazily - a class and a Rust message, no Ruby object - so making one
/// allocates nothing Ruby could raise `NoMemoryError` from: the callers build
/// it while they still own the parse error, the context and the AST, which a
/// raise here would leak. magnus makes the exception (a UTF-8 message) when
/// it raises.
pub fn xpath_error(err: &XPathError) -> Error {
    let class = match err.status {
        ErrorKind::Syntax => EXC_XPATH_SYNTAX_ERROR.exception(),
        ErrorKind::Limit => EXC_XPATH_LIMIT_EXCEEDED.exception(),
        ErrorKind::NotImplemented
        | ErrorKind::Type
        | ErrorKind::Runtime
        | ErrorKind::Internal
        | ErrorKind::Oom => EXC_ERROR.exception(),
    };
    Error::new(
        class,
        err.message()
            .unwrap_or("XPath evaluation failed")
            .to_owned(),
    )
}

/// A context bound to whichever backend the receiver's document is.
///
/// The glue holds one of these. It is the engine's document-free [`Session`] -
/// the context node, the registrations, the caps and the mode, which is all
/// that persists between evaluates - and nothing of the document: an
/// `XPathContext` keeps its `Cx` for the Document's whole life, while the
/// mutators between two evaluates edit that document, so [`run`](Self::run)
/// borrows it from the Document for one evaluate at a time. Everything but
/// the evaluate is the same operation on either backend, so `Cx` derefs to the
/// session and the backend is chosen in exactly one place.
pub struct Cx {
    session: Session,
}

impl Cx {
    /// Evaluate `ast` over `document`, borrowed for this call alone: a read
    /// guard, released when the engine returns an owned value. No Ruby runs
    /// while it is held - Makiri runs no custom-function handler. The outer
    /// `Err` is a read the document refused.
    #[allow(clippy::result_large_err)]
    fn run(
        &self,
        document: Value,
        ast: &Ast,
        answer: Answer,
    ) -> Result<Result<XPathValue, XPathError>, Error> {
        Ok(match DocKind::of(document) {
            DocKind::Xml => {
                let doc = xml_read(&document)?;
                run_on(&self.session, &*doc, ast, answer)
            }
            DocKind::Html => {
                let doc = html_read(&document)?;
                run_on(
                    &self.session,
                    crate::html_dom::xpath::dom(doc.parsed()),
                    ast,
                    answer,
                )
            }
        })
    }
}

/// One evaluate of `ast` over `doc` under `session`.
#[allow(clippy::result_large_err)]
fn run_on<'d, D: Dom<'d>>(
    session: &Session,
    doc: D,
    ast: &Ast,
    answer: Answer,
) -> Result<XPathValue, XPathError> {
    match answer {
        Answer::First => session.evaluate_first(doc, ast),
        Answer::All => session.evaluate(doc, ast),
    }
}

impl core::ops::Deref for Cx {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl core::ops::DerefMut for Cx {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

/// Build the context a query on `rb_node` runs under, bound to `document`.
///
/// Neither branch builds an index here: HTML's element index is built by the
/// first `//tag` that asks it for a bucket (`HtmlDom::name_bucket`), and the
/// XML branch's name index hangs off the document.
///
/// The context keeps nothing of the document: the caller holds `document`
/// for as long as the context lives, and each evaluate borrows it
/// ([`Cx::run`]).
pub fn context_for(rb_node: Value, document: Value) -> Result<Cx, Error> {
    DOC_TYPE.get(&document)?; /* TypeError for a non-Document */
    if DocKind::of(document) == DocKind::Xml {
        /* The context NODE: `xml_node_unwrap` resolves a Document receiver to
         * its arena's document node, and any other node to itself. */
        let node = xml_node_unwrap(rb_node)?;
        return Ok(Cx {
            session: Session::new(Some(Token::xml(node.to_token()))),
        });
    }

    let (key, _) = html_node_key(rb_node)?;
    /* Resolved against `document` - checked, not assumed, to own the node -
     * and kept as the context's token, which `document` keeps alive for as
     * long as the context lives. */
    let node = with_html_node(document, key, |n| <HtmlDom<'_> as Dom<'_>>::token(n))?;
    Ok(Cx {
        session: Session::new(Some(node)),
    })
}

/// Parse `text` under `limits`, on a budget of its own - every parse, cached
/// or not, charges one made from the caps - answering the budget's error on
/// failure. The one parse step for [`parse_query`] and `XPathContext`'s cache.
#[allow(clippy::result_large_err)]
fn parse_with_limits(
    limits: crate::xpath::limits::Limits,
    text: crate::text::VerifiedText<'_>,
) -> Result<Box<Ast>, XPathError> {
    let mut budget = Budget::with_limits(limits);
    crate::xpath::parse::parse_owned(text, &mut budget).map_err(|_| budget.take_error())
}

/// Parse `expr` for one query under `cx`'s caps; a failure is the parse's
/// error as the exception.
pub fn parse_query(cx: &Cx, expr: Value) -> Result<Box<Ast>, Error> {
    let ev = ruby_verified_text(expr, "XPath expression")?;
    /* `ev` owns a copy of the String's bytes; the parse reads that copy. */
    let parsed = parse_with_limits(cx.limits(), ev.text());
    /* The copy is freed before the error is built, as `rake unsafe:boundaries`
     * asks of every argument copy. */
    drop(ev);
    parsed.map_err(|error| xpath_error(&error))
}

/// The compiled expression one evaluation runs: the query's own, or one an
/// `XPathContext`'s cache keeps - which frees nothing before the context goes,
/// so it outlives an evaluation on that context.
pub struct QueryAst(QueryAstRepr);

enum QueryAstRepr {
    Owned(Box<Ast>),
    Cached(NonNull<Ast>),
}

impl QueryAst {
    /// An AST the query owns, freed when this is.
    pub fn owned(ast: Box<Ast>) -> Self {
        QueryAst(QueryAstRepr::Owned(ast))
    }

    /// An AST someone else owns.
    ///
    /// # Safety
    /// `ast` stays alive and unchanged until this value is dropped - as an
    /// `XPathContext`'s cached AST does for an evaluation on that context.
    unsafe fn cached(ast: &Ast) -> Self {
        QueryAst(QueryAstRepr::Cached(NonNull::from(ast)))
    }

    fn get(&self) -> &Ast {
        match &self.0 {
            QueryAstRepr::Owned(ast) => ast,
            // SAFETY: alive and unchanged for this value's life, per `cached`.
            QueryAstRepr::Cached(ast) => unsafe { ast.as_ref() },
        }
    }
}

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/// An engine value as a Ruby object; `document` is the keepalive a node-set
/// is wrapped under. Every allocation's failure, and a push the set refuses
/// (its cap), comes back as `Err`.
///
/// The one conversion for a whole value. `at_xpath` is the exception by design: its
/// [`query_result`] fast path wraps only a node-set's first node, through the
/// same wrap a set's read-back uses, and never builds the set.
fn val_to_ruby(v: ValRef<'_>, document: Value) -> Result<Value, Error> {
    Ok(match v {
        ValRef::NodeSet(set) => {
            let (rb, fill) = node_set_with_fill(document)?;
            fill.extend(set.as_slice().iter().map(|&n| NodeWord::of_token(n)))?;
            rb
        }
        /* The bytes are the value's own, and valid UTF-8 - the engine builds
         * a Text only from input the text contract has passed. */
        ValRef::String(t) => crate::bridge::string::utf8_str(t.as_slice())?,
        ValRef::Number(d) => crate::bridge::build::float(d)?,
        ValRef::Boolean(b) => crate::bridge::ruby::boolean(b),
    })
}

/// An evaluation result as a Ruby object. Converting consumes the value, which
/// frees its node-set array or string; `document` is the keepalive for a
/// node-set.
fn value_to_ruby(v: XPathValue, document: Value) -> Result<Value, Error> {
    val_to_ruby(v.get(), document)
}

/* ------------------------------------------------------------------ */
/* the query path every entry point shares                            */
/* ------------------------------------------------------------------ */
/* `Node#xpath` / `#at_xpath` for both representations, the XML `#css` family
 * and `XPathContext#evaluate` all parse, then end in `evaluate_to_ruby`; they
 * differ only in how the context is built and who owns it. The XML `matches?`
 * reads a boolean from `evaluate_query` and converts nothing. */

/// What a query answers: every result, or - for `at_xpath` / `at_css` - the
/// first node of a node-set, which also lets the engine stop at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    All,
    First,
}

/// Evaluate `ast` under `ctx`. [`Answer::First`] takes the `at_xpath` fast
/// path.
pub fn evaluate_query(
    ctx: &Cx,
    ast: &Ast,
    document: Value,
    answer: Answer,
) -> Result<XPathValue, Error> {
    let had_order = crate::bridge::wrapper::has_kept_order(document);
    let result = ctx.run(document, ast, answer);
    /* A document-order index the evaluate built is the document's now. */
    crate::bridge::wrapper::account_kept_order(document, had_order);
    result?.map_err(|error| xpath_error(&error))
}

/// Evaluate `ast` under `ctx` and convert the value for Ruby: every query that
/// answers a Ruby value - `Node#xpath` / `#at_xpath`, the XML `#css` family,
/// `XPathContext#evaluate` - ends here.
///
/// The AST and the context - owned (`Cx`, a query's throwaway) or borrowed
/// (`&Cx`, an `XPathContext`'s) - are dropped BEFORE the conversion, which
/// allocates Ruby objects and so may raise or collect: the value owns its data
/// and references neither, so nothing is held that a raise would leak.
pub fn evaluate_to_ruby<C: core::borrow::Borrow<Cx>>(
    ctx: C,
    ast: QueryAst,
    document: Value,
    answer: Answer,
) -> Result<Value, Error> {
    let value = evaluate_query(ctx.borrow(), ast.get(), document, answer);
    drop(ast);
    drop(ctx);
    query_result(value?, document, answer)
}

/// A query's value as Ruby, and for `at_xpath` the first node of a node-set.
///
/// Callers free the AST and any context they own BEFORE this: the value owns
/// its data and references neither.
pub fn query_result(value: XPathValue, document: Value, answer: Answer) -> Result<Value, Error> {
    if answer == Answer::First {
        if let ValRef::NodeSet(set) = value.get() {
            /* Only the first node is wrapped - no NodeSet, no `#first` call.
             * The pointer is the document's, so the value is freed before the
             * wrap, which allocates and so can raise past this frame. */
            let first = set.as_slice().first().map(|&n| NodeWord::of_token(n));
            drop(value);
            return match first {
                // SAFETY: a node the query found in `document`.
                Some(n) => unsafe { wrap_doc_node(DocKind::of(document), n, document) },
                None => Ok(crate::bridge::ruby::nil()),
            };
        }
    }
    value_to_ruby(value, document)
}
