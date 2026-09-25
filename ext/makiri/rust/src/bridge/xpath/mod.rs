//! The Ruby <-> XPath engine seam: which backend a query runs on, building the
//! engine context for a Ruby node or document, and the safe dispatch the glue
//! calls.
//!
//! The engine (`xpath/`) is generic over `Dom` and names no representation, and
//! `glue/query.rs` must not name a Lexbor or XML type either. This module sits
//! between them: it is the one place that knows both backends exist, and it
//! turns a Ruby node into the right context with the raw-pointer work the
//! backend's lifetime contract needs.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{prelude::*, Error, Value};

use crate::bridge::html::html_node_unwrap;
use crate::bridge::node_set::{node_set_with_fill, PushError};
use crate::bridge::ruby::VALUE;
use crate::bridge::string::{ruby_str_from_utf8, ruby_verified_text};
use crate::bridge::wrapper::{doc_content, html_doc_unwrap, with_html_parsed_known, Content};
use crate::bridge::wrapper::{wrap_doc_node, DocKind, NodeWord};
use crate::bridge::xml::xml_node_unwrap;
use crate::init::EXC_ERROR;
pub use crate::init::{CLASS_XPATH_CONTEXT, EXC_XPATH_LIMIT_EXCEEDED, EXC_XPATH_SYNTAX_ERROR};
use crate::lexbor::adapter::post_parse::HtmlParsed;
use crate::token::{Kind, Token};
use crate::xml::model::Document as XmlDoc;
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Resolver, Session, XPathValue};
use crate::xpath::dom::Dom;
use crate::xpath::limits::Budget;
use crate::xpath::msg::{Error as XPathError, Status};
use crate::xpath::value::ValRef;
use core::ptr::NonNull;

mod context_object;
mod handler;

use crate::bridge::ruby::is_kind_of;
pub use context_object::XPathCtx;
use handler::Bridge;

/// An engine error as the Ruby exception it maps to.
///
/// Returned rather than raised: `rb_raise` longjmps past every Rust destructor
/// on the way (see `glue/mod.rs`), so each caller hands this back as `Err` and
/// magnus raises once its frames - and the context they own - are gone.
pub fn xpath_error(err: &XPathError) -> Error {
    let class = match err.status {
        Status::Syntax => EXC_XPATH_SYNTAX_ERROR.exception(),
        Status::Limit => EXC_XPATH_LIMIT_EXCEEDED.exception(),
        Status::NotImplemented
        | Status::Type
        | Status::Runtime
        | Status::Internal
        | Status::Oom => EXC_ERROR.exception(),
    };
    let ruby = magnus::Ruby::get_with(class);
    let msg = ruby.enc_str_new(
        err.message().unwrap_or("XPath evaluation failed"),
        ruby.utf8_encoding(),
    );
    match class.new_instance((msg,)) {
        Ok(e) => Error::from(e),
        Err(e) => e,
    }
}

/// The document a [`Cx`] evaluates, as the pointer the Document wrapper owns.
///
/// A pointer, not a reference, on purpose: a context outlives any one call - an
/// `XPathContext` keeps its `Cx` for the Document's whole life - while the
/// mutators between two evaluates write the same arena through `&mut`. A
/// `&Document` kept across such a write is undefined behaviour even when no
/// read overlaps it, so the reference is taken per evaluate and dropped with it
/// (see [`Cx::run`]); what persists holds no borrow.
#[derive(Clone, Copy)]
enum DocPtr {
    /// A Lexbor document's parse handle.
    Html(NonNull<HtmlParsed>),
    /// A Makiri XML arena.
    Xml(NonNull<XmlDoc>),
}

/// A context bound to whichever backend the receiver's document is.
///
/// The glue holds one of these. It is the engine's document-free [`Session`] -
/// the context node, the registrations, the caps and the mode, which is all
/// that persists between evaluates - plus a pointer to the document, which
/// [`run`](Self::run) lends to one evaluate at a time. Everything but the
/// evaluate is the same operation on either backend, so `Cx` derefs to the
/// session and the backend is chosen in exactly one place.
pub struct Cx {
    session: Session,
    doc: DocPtr,
}

impl Cx {
    /// Which backend this context walks, for minting a node token.
    pub fn token_kind(&self) -> Kind {
        match self.doc {
            DocPtr::Html(_) => Kind::Html,
            DocPtr::Xml(_) => Kind::Xml,
        }
    }

    /// Evaluate `ast` over the document, borrowed for this call alone.
    ///
    /// The caller holds the Document for the call, and the document does not
    /// change while it runs: without a handler no Ruby runs, and with one
    /// [`evaluate_query`]'s `Bridge` holds the document's mutation guard. That
    /// is the whole span of the borrow taken here - it ends when the engine
    /// returns an owned value.
    #[allow(clippy::result_large_err)]
    fn run(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
        answer: Answer,
    ) -> Result<XPathValue, XPathError> {
        match self.doc {
            DocPtr::Xml(p) => {
                // SAFETY: the arena of the Document the caller holds, unchanged
                // for this call (above); the borrow ends with it.
                let doc: &XmlDoc = unsafe { p.as_ref() };
                run_on(&self.session, doc, ast, handler, answer)
            }
            DocPtr::Html(p) => {
                // SAFETY: as above, for the parse handle.
                let dom = unsafe { crate::lexbor::xpath::dom(p) };
                run_on(&self.session, dom, ast, handler, answer)
            }
        }
    }
}

/// One evaluate of `ast` over `doc` under `session`.
#[allow(clippy::result_large_err)]
fn run_on<'d, D: Dom<'d>>(
    session: &Session,
    doc: D,
    ast: &Ast,
    handler: Option<&dyn Resolver>,
    answer: Answer,
) -> Result<XPathValue, XPathError> {
    match answer {
        Answer::First => session.evaluate_first(doc, ast, handler),
        Answer::All => session.evaluate(doc, ast, handler),
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
/// The HTML branch builds the element index up front, so `//tag` is answered
/// without a tree walk and an allocation failure raises here rather than on the
/// first evaluate. The XML branch's name index hangs off the document.
///
/// The context keeps a pointer to the document, not a borrow: the caller holds
/// `document` for as long as the context lives, and each evaluate reborrows it
/// ([`Cx::run`]).
pub fn context_for(rb_node: Value, document: Value) -> Result<Cx, Error> {
    let content = doc_content(document)?;

    if let Content::Xml(xdoc) = content {
        /* The context NODE: `xml_node_unwrap` resolves a Document receiver to
         * its arena's document node, and any other node to itself. */
        let node = xml_node_unwrap(rb_node)?;
        return Ok(Cx {
            session: Session::new(Some(Token::xml(node.to_token()))),
            doc: DocPtr::Xml(xdoc),
        });
    }

    let raw = html_node_unwrap(rb_node)?;
    // SAFETY: `html_node_unwrap` returned a live node of `document`.
    let node = unsafe { Token::html(raw.as_ptr()) };
    /* TypeError for a Document that is not HTML. */
    html_doc_unwrap(document)?;
    let Content::Html(parsed) = content else {
        return Err(makiri_error("XPath context with no document"));
    };
    /* Built up front, so an allocation failure raises here rather than on the
     * first evaluate. Each evaluate still reads the index afresh from the
     * handle, which rebuilds it after a mutation. */
    if with_html_parsed_known(document, |p| p.ensure_dom_index().is_err()) {
        return Err(makiri_error("failed to build the element index for XPath"));
    }
    Ok(Cx {
        session: Session::new(Some(node)),
        doc: DocPtr::Html(parsed),
    })
}

/// Parse `expr` for one query under `cx`'s caps, on a budget of the query's own;
/// a failure is that budget's error as the exception.
pub fn parse_query(cx: &Cx, expr: Value) -> Result<Box<Ast>, Error> {
    let ev = ruby_verified_text(expr, "XPath expression")?;
    let mut budget = Budget::with_limits(cx.limits());
    /* `ev` holds the String rooted and locked; `text`'s borrow keeps it live for the
     * parse. */
    let parsed = crate::xpath::parse::parse_owned(ev.text(), &mut budget);
    /* No borrowed bytes across the exception's allocation. */
    drop(ev);
    parsed.map_err(|_| xpath_error(&budget.take_error()))
}

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/// An engine value as a Ruby object, NOT protected: every branch allocates,
/// and an allocation can raise, so each caller runs this under `protect`. A
/// NodeSet push the set refuses (its cap) comes back as `Err`.
///
/// The one conversion for a whole value, for a query's result and for a
/// handler's arguments alike, so the two cannot drift apart in how a string or
/// a node-set reaches Ruby. `at_xpath` is the exception by design: its
/// [`query_result`] fast path wraps only a node-set's first node, through the
/// same wrap a set's read-back uses, and never builds the set.
///
/// # Safety
/// Under `protect`; `document` is the keepalive a node-set is wrapped under.
unsafe fn val_to_ruby(v: ValRef<'_>, document: Value) -> Result<VALUE, PushError> {
    Ok(match v {
        ValRef::NodeSet(set) => {
            let (rb, fill) = node_set_with_fill(document);
            for &n in set.as_slice() {
                fill.push(NodeWord::of_token(n))?;
            }
            rb.as_raw()
        }
        /* The bytes are the value's own, and valid UTF-8 - the engine builds
         * a Text only from input the text contract has passed. */
        ValRef::String(t) => ruby_str_from_utf8(t.as_slice()),
        ValRef::Number(d) => crate::bridge::ruby::float(d).as_raw(),
        ValRef::Boolean(b) => crate::bridge::ruby::boolean(b).as_raw(),
    })
}

/// An evaluation result as a Ruby object. Converting consumes the value, which
/// frees its node-set array or string; `document` is the keepalive for a
/// node-set.
///
/// Under one `protect` per result, not per node, reading `v` by reference: a
/// raise comes back as `Err`, and `v` is still freed on the way out rather than
/// skipped by the longjmp.
fn value_to_ruby(v: XPathValue, document: Value) -> Result<Value, Error> {
    /* A refused push cannot leave `protect` through `?`, so it is carried out. */
    let mut refused = None;
    let converted = crate::bridge::ruby::protect_value(|| {
        // SAFETY: under `protect`, as `val_to_ruby` asks.
        match unsafe { val_to_ruby(v.get(), document) } {
            Ok(rb) => rb,
            Err(e) => {
                refused = Some(e);
                crate::bridge::ruby::nil().as_raw()
            }
        }
    });
    drop(v);
    let converted = converted?;
    if let Some(e) = refused {
        return Err(e.into());
    }
    Ok(converted)
}

/* ------------------------------------------------------------------ */
/* the query path every entry point shares                            */
/* ------------------------------------------------------------------ */
/* `Node#xpath` / `#at_xpath` for both representations, the XML `#css` family
 * and `XPathContext#evaluate` all run parse -> evaluate -> convert through
 * these three; they differ only in how the context is built and who owns it. */

/// Evaluate `ast` under `ctx`, with `handler` (if any) answering unknown
/// functions for this evaluation only. [`Answer::First`] takes the `at_xpath` fast
/// path.
/// What a query answers: every result, or - for `at_xpath` / `at_css` - the
/// first node of a node-set, which also lets the engine stop at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    All,
    First,
}

pub fn evaluate_query(
    ctx: &Cx,
    ast: &Ast,
    handler: Option<Value>,
    document: Value,
    answer: Answer,
) -> Result<XPathValue, Error> {
    /* A handler runs Ruby mid-walk, so for as long as one can, the document
     * refuses to be changed: the bridge holds that guard, and lives on this
     * frame for this evaluate alone. */
    let bridge = match handler {
        None => None,
        Some(handler) => Some(Bridge {
            handler: handler.as_raw(),
            document: document.as_raw(),
            kind: ctx.token_kind(),
            _reading: crate::bridge::wrapper::DocumentEvaluation::enter(document)?,
        }),
    };
    let resolver = bridge.as_ref().map(|b| b as &dyn Resolver);
    ctx.run(ast, resolver, answer)
        .map_err(|error| xpath_error(&error))
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
            return Ok(match first {
                // SAFETY: a node the query found in `document`.
                Some(n) => unsafe { wrap_doc_node(DocKind::of(document), n, document) },
                None => crate::bridge::ruby::nil(),
            });
        }
    }
    value_to_ruby(value, document)
}
