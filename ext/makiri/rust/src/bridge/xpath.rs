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

use magnus::{prelude::*, Error, Value};

use crate::bridge::lexbor::{
    doc_parsed, html_doc_unwrap, html_node_unwrap, parsed_xml_doc, xml_node_unwrap,
};
use crate::bridge::string::ruby_verified_text;
use crate::init::{CLASS_XML_DOCUMENT, EXC_ERROR, EXC_XPATH_LIMIT_EXCEEDED, EXC_XPATH_SYNTAX_ERROR};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Context, ContextError, Resolver, XPathValue};
use crate::xpath::limits::{Budget, Limits};
use crate::xpath::msg::{Error as XPathError, XP_ERR_LIMIT, XP_ERR_SYNTAX};
use crate::xpath::token::Token;

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
    let bytes = err.message().unwrap_or(c"XPath evaluation failed").to_bytes();
    let msg = ruby.enc_str_new(bytes, ruby.utf8_encoding());
    match class.new_instance((msg,)) {
        Ok(e) => Error::from(e),
        Err(e) => e,
    }
}

/// A context bound to whichever backend the receiver's document is.
///
/// The glue holds one of these; the engine sees only the concrete
/// `Context<'d, D>` inside.
pub enum Cx {
    /// A Lexbor document.
    #[cfg(feature = "lexbor")]
    Html(Context<'static, crate::lexbor::xpath::HtmlDom<'static>>),
    /// A Makiri XML arena, lent for `'static` because Ruby, not a Rust borrow,
    /// keeps the document alive for as long as the context lives.
    Xml(Context<'static, &'static crate::xml::model::Document>),
}

impl Cx {
    /// The limits a run under this context starts from.
    pub fn limits(&self) -> Limits {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.limits(),
            Cx::Xml(cx) => cx.limits(),
        }
    }

    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub fn lax(&self) -> bool {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.lax(),
            Cx::Xml(cx) => cx.lax(),
        }
    }

    pub fn set_lax(&mut self, lax: bool) {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.set_lax(lax),
            Cx::Xml(cx) => cx.set_lax(lax),
        }
    }

    /// True while an evaluate is in progress on this context.
    pub fn is_evaluating(&self) -> bool {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.is_evaluating(),
            Cx::Xml(cx) => cx.is_evaluating(),
        }
    }

    /// Rebind the context node; refused while an evaluate runs.
    pub fn set_context_node(&self, node: Token) -> Result<(), ContextError> {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.set_context_node(node),
            Cx::Xml(cx) => cx.set_context_node(node),
        }
    }

    /// Bind `prefix` to `uri`, replacing an earlier binding.
    pub fn register_ns(&self, prefix: &[u8], uri: &[u8]) -> Result<(), ContextError> {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.register_ns(prefix, uri),
            Cx::Xml(cx) => cx.register_ns(prefix, uri),
        }
    }

    /// Bind the unprefixed `$name`, replacing an earlier binding.
    pub fn register_variable(&self, name: &[u8], value: &[u8]) -> Result<(), ContextError> {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.register_variable(name, value),
            Cx::Xml(cx) => cx.register_variable(name, value),
        }
    }

    /// Evaluate `ast` under this context.
    #[allow(clippy::result_large_err)]
    pub fn evaluate(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, XPathError> {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.evaluate(ast, handler),
            Cx::Xml(cx) => cx.evaluate(ast, handler),
        }
    }

    /// [`evaluate`](Self::evaluate) through the `at_xpath` first-match fast path.
    #[allow(clippy::result_large_err)]
    pub fn evaluate_first(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, XPathError> {
        match self {
            #[cfg(feature = "lexbor")]
            Cx::Html(cx) => cx.evaluate_first(ast, handler),
            Cx::Xml(cx) => cx.evaluate_first(ast, handler),
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
                return Err(Error::new(
                    EXC_ERROR.exception(),
                    "XPath context with no document",
                ));
            }
            /* The context NODE is the document node for a Document receiver,
             * else the node itself. */
            let node = if rb_node.is_kind_of(CLASS_XML_DOCUMENT.class()) {
                Token::from_ptr(
                    (*(xdoc as *mut crate::xml::model::Doc))
                        .doc_node()
                        .to_token() as *mut c_void,
                )
            } else {
                Token::from_ptr(xml_node_unwrap(rb_node)?)
            };
            // SAFETY: the XML arena behind `document`, live for `'static` by the
            // caller's keepalive.
            let doc: &'static crate::xml::model::Document = &*xdoc;
            return Ok(Cx::Xml(Context::new(doc, node)));
        }

        let node = Token::from_ptr(html_node_unwrap(rb_node)?.as_ptr());
        /* TypeError for a Document that is not HTML. */
        html_doc_unwrap(document)?;
        /* Built up front, so an allocation failure raises here rather than on the
         * first evaluate. Each evaluate still reads the index afresh from the
         * handle, which rebuilds it after a mutation. */
        if (*parsed).dom_index().is_none() {
            return Err(Error::new(
                EXC_ERROR.exception(),
                "failed to build attribute index for XPath",
            ));
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
