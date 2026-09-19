//! The Ruby <-> XPath engine seam: which backend a query runs on, building the
//! engine context for a Ruby node or document, and the safe dispatch the glue
//! calls.
//!
//! The engine (`xpath/`) is generic over `Dom` and names no representation, and
//! `glue/xpath.rs` must not name a Lexbor or XML type either. This module sits
//! between them: it is the one place that knows both backends exist, and it
//! turns a Ruby node into the right context with the raw-pointer work the
//! backend's lifetime contract needs.

#![allow(unsafe_code)]

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::value::ReprValue;
use magnus::{prelude::*, Error, Value};

use crate::bridge::html::html_node_unwrap;
use crate::bridge::node_set::{node_set_with_fill, PushError};
use crate::bridge::ruby::VALUE;
use crate::bridge::string::{ruby_str_from_utf8, ruby_verified_text};
use crate::bridge::wrapper::{doc_parsed, html_doc_unwrap, parsed_xml_doc};
use crate::bridge::xml::xml_node_unwrap;
use crate::init::{CLASS_NODE_SET, CLASS_XML_DOCUMENT, EXC_ERROR};
pub use crate::init::{CLASS_XPATH_CONTEXT, EXC_XPATH_LIMIT_EXCEEDED, EXC_XPATH_SYNTAX_ERROR};
use crate::token::{Kind, Token};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Context, QueryContext, Resolver, XPathValue};
use crate::xpath::limits::Budget;
use crate::xpath::msg::{Error as XPathError, XP_ERR_LIMIT, XP_ERR_SYNTAX};
use crate::xpath::value::ValRef;

mod context_object;
mod handler;

use crate::bridge::ruby::is_kind_of;
pub use context_object::{init_xpath_context, ns_matching_lax};
use handler::Bridge;

/// An engine error as the Ruby exception it maps to.
///
/// Returned rather than raised: `rb_raise` longjmps past every Rust destructor
/// on the way (see `glue/mod.rs`), so each caller hands this back as `Err` and
/// magnus raises once its frames - and the context they own - are gone.
pub fn xpath_error(err: &XPathError) -> Error {
    let class = match err.status {
        XP_ERR_SYNTAX => EXC_XPATH_SYNTAX_ERROR.exception(),
        XP_ERR_LIMIT => EXC_XPATH_LIMIT_EXCEEDED.exception(),
        _ => EXC_ERROR.exception(),
    };
    let ruby = magnus::Ruby::get_with(class);
    /* The message's bytes as they are, tagged UTF-8 - not a lossy copy. */
    let bytes = err
        .message()
        .unwrap_or(c"XPath evaluation failed")
        .to_bytes();
    let msg = ruby.enc_str_new(bytes, ruby.utf8_encoding());
    match class.new_instance((msg,)) {
        Ok(e) => Error::from(e),
        Err(e) => e,
    }
}

/// A context bound to whichever backend the receiver's document is.
///
/// The glue holds one of these; the engine sees only the concrete
/// `Context<'d, D>` inside. Everything but the token kind is the same operation
/// on either backend, so `Cx` derefs to [`QueryContext`] and the backend is
/// chosen in exactly one place.
pub enum Cx {
    /// A Lexbor document.
    Html(Context<'static, crate::lexbor::xpath::HtmlDom<'static>>),
    /// A Makiri XML arena, lent for `'static` because Ruby, not a Rust borrow,
    /// keeps the document alive for as long as the context lives.
    Xml(Context<'static, &'static crate::xml::model::Document>),
}

impl Cx {
    /// Which backend this context walks, for minting a node token.
    pub fn token_kind(&self) -> Kind {
        match self {
            Cx::Html(_) => Kind::Html,
            Cx::Xml(_) => Kind::Xml,
        }
    }
}

impl core::ops::Deref for Cx {
    type Target = dyn QueryContext;
    fn deref(&self) -> &(dyn QueryContext + 'static) {
        match self {
            Cx::Html(cx) => cx,
            Cx::Xml(cx) => cx,
        }
    }
}

impl core::ops::DerefMut for Cx {
    fn deref_mut(&mut self) -> &mut (dyn QueryContext + 'static) {
        match self {
            Cx::Html(cx) => cx,
            Cx::Xml(cx) => cx,
        }
    }
}

/// Build the context a query on `rb_node` runs under, bound to `document`.
///
/// The HTML branch builds the attr->owner index up front, so the engine's parent
/// and ancestor axes and its document-order sort see attribute owners, and hands
/// over the element index so `//tag` is answered without a tree walk. The XML
/// branch needs neither: the custom node links attributes to their owner
/// directly, and its name index hangs off the document.
///
/// The context is `'static` because Ruby, not a Rust borrow, keeps the document
/// alive: the caller holds `document` for as long as the context lives. The
/// document does not change while an evaluate runs: without a handler no Ruby
/// runs, and with one the glue's `Bridge` holds the document's mutation guard.
pub fn context_for(rb_node: Value, document: Value) -> Result<Cx, Error> {
    let parsed = doc_parsed(document)?;

    // SAFETY: the handle of `document`, which the caller holds for as long as
    // the context it gets back.
    unsafe {
        if (*parsed).is_xml() {
            let xdoc = parsed_xml_doc(parsed);
            if xdoc.is_null() {
                return Err(makiri_error("XPath context with no document"));
            }
            /* The context NODE is the document node for a Document receiver,
             * else the node itself. */
            let node = if rb_node.is_kind_of(CLASS_XML_DOCUMENT.class()) {
                Token::xml(
                    (*(xdoc as *mut crate::xml::model::Doc))
                        .doc_node()
                        .to_token(),
                )
            } else {
                Token::xml(xml_node_unwrap(rb_node)? as usize)
            };
            // SAFETY: the XML arena behind `document`, live for `'static` by the
            // caller's keepalive.
            let doc: &'static crate::xml::model::Document = &*xdoc;
            return Ok(Cx::Xml(Context::new(doc, node)));
        }

        /* SAFETY: `html_node_unwrap` returned a live node of `document`, and
         * this block already reasons under the `context_for` contract. */
        let node = Token::html(html_node_unwrap(rb_node)?.as_ptr());
        /* TypeError for a Document that is not HTML. */
        html_doc_unwrap(document)?;
        /* Built up front, so an allocation failure raises here rather than on the
         * first evaluate. Each evaluate still reads the index afresh from the
         * handle, which rebuilds it after a mutation. */
        if (*parsed).dom_index().is_none() {
            return Err(makiri_error("failed to build attribute index for XPath"));
        }
        let cx = crate::lexbor::xpath::context(parsed, node).map_err(|e| xpath_error(&e))?;
        Ok(Cx::Html(cx))
    }
}

/// Parse `expr` for one query under `cx`'s caps, on a budget of the query's own;
/// a failure is that budget's error as the exception.
pub fn parse_query(cx: &Cx, expr: Value) -> Result<Box<Ast>, Error> {
    let ev = ruby_verified_text(expr, c"XPath expression")?;
    let mut budget = Budget::with_limits(cx.limits());
    /* `ev` holds the String rooted; `as_verified`'s borrow keeps it live for the
     * parse. */
    let parsed = crate::xpath::parse::parse_owned(ev.as_verified(), &mut budget);
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
/// The one conversion, for a query's result and for a handler's arguments
/// alike, so the two cannot drift apart in how a string or a node-set reaches
/// Ruby.
///
/// # Safety
/// Under `protect`; `document` is the keepalive a node-set is wrapped under.
unsafe fn val_to_ruby(v: ValRef<'_>, document: Value) -> Result<VALUE, PushError> {
    Ok(match v {
        ValRef::NodeSet(set) => {
            let (rb, fill) = node_set_with_fill(document);
            for &n in set.as_slice() {
                fill.push(n.as_ptr())?;
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

/// The token for `raw`, a node of a document walked by backend `kind`. `None`
/// for [`Kind::Null`], which names no document - the one place both callers
/// mint a node token, so neither can let a null kind slip through as XML.
///
/// # Safety
/// `raw` must be a live node of a document of backend `kind`.
unsafe fn node_token(kind: Kind, raw: *mut c_void) -> Option<Token> {
    match kind {
        Kind::Html => Some(Token::html(raw)),
        Kind::Xml => Some(Token::xml(raw as usize)),
        Kind::Null => None,
    }
}

/* ------------------------------------------------------------------ */
/* the query path every entry point shares                            */
/* ------------------------------------------------------------------ */
/* `Node#xpath` / `#at_xpath` for both representations, the XML `#css` family
 * and `XPathContext#evaluate` all run parse -> evaluate -> convert through
 * these three; they differ only in how the context is built and who owns it. */

/// Evaluate `ast` under `ctx`, with `handler` (nil for none) answering unknown
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
    handler: Value,
    document: Value,
    answer: Answer,
) -> Result<XPathValue, Error> {
    /* A handler runs Ruby mid-walk, so for as long as one can, the document
     * refuses to be changed: the bridge holds that guard, and lives on this
     * frame for this evaluate alone. */
    let bridge = if handler.is_nil() {
        None
    } else {
        Some(Bridge {
            handler: handler.as_raw(),
            document: document.as_raw(),
            kind: ctx.token_kind(),
            _reading: crate::bridge::wrapper::DocumentEvaluation::enter(document)?,
        })
    };
    let resolver = bridge.as_ref().map(|b| b as &dyn Resolver);
    let result = if answer == Answer::First {
        ctx.evaluate_first(ast, resolver)
    } else {
        ctx.evaluate(ast, resolver)
    };
    result.map_err(|error| xpath_error(&error))
}

/// A query's value as Ruby, and for `at_xpath` the first node of a node-set.
///
/// Callers free the AST and any context they own BEFORE this: the value owns
/// its data and references neither.
pub fn query_result(value: XPathValue, document: Value, answer: Answer) -> Result<Value, Error> {
    let result = value_to_ruby(value, document)?;
    if answer == Answer::First && is_kind_of(result, &CLASS_NODE_SET) {
        return result.funcall("first", ());
    }
    Ok(result)
}
