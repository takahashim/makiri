//! The Ruby boundary for the native XML reader (glue/ruby_xml.c).
//!
//!   `Makiri::XML::Document.parse` / `Makiri::XML(source)`
//!   `#xpath` / `#at_xpath`, and the private `_css` / `_at_css` / `_css_matches`
//!   `Document.new`, `#fragment`, `DocumentFragment.parse`, `#root`,
//!   `#internal_subset`
//!
//! Unlike the HTML glue, nothing here touches Lexbor: the engine on the other
//! side is Makiri's own XML reader and its own XPath evaluator, both of which
//! are already Rust. So there is no vendored layout in reach, and the C types
//! that do appear are ours.
//!
//! # This is where the GVL is released
//!
//! `Document.parse` is the production form of the spike in §11 of the rewrite
//! plan: copy the decoded source into a private buffer while holding the GVL,
//! release it, and touch nothing Ruby inside. The buffer is copied BEFORE the
//! Ruby wrapper is allocated, so no GC can run between obtaining the decoded
//! String and copying it, and the parse cannot then race GC or compaction on
//! that String's backing store.
//!
//! Fragment parsing deliberately does NOT release the GVL: a fragment is small,
//! and an existing document's arena must never be mutated with the GVL down.

#![forbid(unsafe_code)]

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;
use magnus::{method, prelude::*, Error, RArray, RHash, RString, Ruby, Value};

use crate::xml::model::{Limits as XmlLimits, NodeId};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::XPathValue;
use crate::xpath::msg::XP_ERR_SYNTAX;

use crate::bridge::lexbor::error_class;

/* The arena ceiling comes from `crate::xml::model` rather than being restated
 * here: that module is the XML engine's own declaration of it. */
use crate::xml::model::MAX_BYTES;

/// `MKR_CSS_DEFAULT_NS_PREFIX` - the synthetic prefix a default namespace
/// arrives under, Nokogiri's convention, so a bare type selector binds to it.
const CSS_DEFAULT_NS_PREFIX: &str = "xmlns";

/// The engine context. Opaque here while C held it; now the real type.
use crate::bridge::xpath::Cx as XPathContext;

/// The default-namespace prefix, or NULL. Declared twice while C held it (once
/// here, once in `css`); the fields matched, but nothing checked that.
use crate::css::CssNs;

use crate::bridge::lexbor::{keepalive_document, wrap_xml_node, xml_node_unwrap};
use crate::bridge::node_set::node_set_new;
use crate::bridge::string::{ruby_verified_text, verify_text};
use crate::bridge::string::ruby_try_verified_text_pair;
use crate::init::{CLASS_DOCUMENT, CLASS_XML_DOCUMENT_FRAGMENT, EXC_CSS_SYNTAX_ERROR, MOD_XML, MOD_XML_NODE_METHODS};

/// Wrap an XML node, typed.
fn wrap_typed_xml_node(node: NodeId, document: Value) -> Value {
    wrap_xml_node(node.to_token() as *mut c_void, document)
}

/// The XML node behind a wrapper, typed. `Err(TypeError)` for an HTML node.
fn typed_xml_node_unwrap(rb_node: Value) -> Result<NodeId, Error> {
    Ok(NodeId::from_token(xml_node_unwrap(rb_node)? as usize))
}

use crate::bridge::xpath::{context_for, parse_query, xpath_error};
use crate::glue::xpath::{evaluate_query, query_result};

/* ------------------------------------------------------------------ */
/* parse                                                              */
/* ------------------------------------------------------------------ */

/// The optional per-parse budget overrides.
///
/// Only `max_bytes` is configurable today; an unknown keyword is an
/// `ArgumentError`, and new budgets join here as they become runtime-settable.
/// It must be a positive Integer - the sign is checked BEFORE the unsigned
/// conversion, because a negative would otherwise wrap into a huge `size_t` and
/// bypass the budget entirely.
fn parse_limits(ruby: &Ruby, h: RHash) -> Result<XmlLimits, Error> {
    let mut limits = XmlLimits { max_bytes: 0 };
    if h.is_empty() {
        return Ok(limits);
    }

    let key = ruby.to_symbol("max_bytes");
    let keys: RArray = h.funcall("keys", ())?;
    for k in keys.into_iter() {
        if k.as_raw() != key.as_raw() {
            return Err(Error::new(
                ruby.exception_arg_error(),
                format!("unknown keyword: {}", k.inspect()),
            ));
        }
    }

    let Some(v) = h.get(key) else {
        return Ok(limits);
    };
    /* An actual Integer, not merely something convertible: the C tested
     * RB_INTEGER_TYPE_P, so a Float (1.5) is a TypeError rather than a silent
     * truncation. */
    let n = magnus::Integer::from_value(v)
        .ok_or_else(|| Error::new(ruby.exception_type_error(), "max_bytes must be an Integer"))?
        .to_i64()?;
    if n <= 0 {
        return Err(Error::new(
            ruby.exception_arg_error(),
            "max_bytes must be positive",
        ));
    }
    limits.max_bytes = n as usize;
    Ok(limits)
}

/// `Makiri::XML::Document.parse(source, max_bytes: nil)`.
///
/// `source` is a String or anything answering `#read` (an IO / File / StringIO).
/// Read a non-UTF-8 file in binary mode so the encoding is autodetected from its
/// BOM or declaration.
fn s_parse(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let scanned = magnus::scan_args::scan_args::<(Value,), (), (), (), RHash, ()>(args)?;
    let (source,) = scanned.required;
    let limits = parse_limits(ruby, scanned.keywords)?;
    let budget = if limits.max_bytes != 0 {
        limits.max_bytes
    } else {
        MAX_BYTES
    };

    /* An IO/File-like source is read first, as the HTML entry does; a String
     * passes straight through. */
    let source = if source.respond_to("read", true)? {
        source.funcall::<_, _, Value>("read", ())?
    } else {
        source
    };

    /* Strict decode, the source copy, the GVL release and the wrapper all live
     * in the seam; here only the budget keywords are read. */
    crate::bridge::xml::parse_xml_document(source, limits, budget)
}

/* ------------------------------------------------------------------ */
/* queries                                                            */
/* ------------------------------------------------------------------ */

/// The (Document VALUE, context node) a query runs against: for a Document the
/// context is the arena's document node, for a node it is that node.
fn query_context(rb_self: Value) -> Result<(Value, NodeId), Error> {
    /* `xml_node_unwrap` is kind-checked - `Err` for a non-XML node - and
     * resolves an XML Document to its document node. */
    let document = keepalive_document(rb_self)?;
    Ok((document, typed_xml_node_unwrap(rb_self)?))
}

/// Register a `{prefix => uri}` Hash onto `ctx` for one query.
///
/// On any bad entry an error is returned, and the caller's owner frees the
/// context - never a partial registration. RSS and Atom live in a default
/// namespace, so a prefix is the strict-mode way to select them.
fn register_namespaces(ruby: &Ruby, ctx: &XPathContext, rb_ns: Option<Value>) -> Result<(), Error> {
    let Some(rb_ns) = rb_ns.filter(|v| !v.is_nil()) else {
        return Ok(());
    };
    let Some(h) = RHash::from_value(rb_ns) else {
        return Err(Error::new(
            ruby.exception_type_error(),
            "namespaces must be a Hash of prefix => uri",
        ));
    };
    let cap = ctx.limits().max_string_bytes;

    let keys: RArray = h.funcall("keys", ())?;
    for k in keys.into_iter() {
        let ks: RString = k.funcall("to_s", ())?;
        let v = h.get(k).unwrap_or_else(|| ruby.qnil().as_value());
        let vs: RString = v.funcall("to_s", ())?;

        /* Both are the Strings `to_s` just returned, and the checks allocate
         * nothing, so the views stay valid through the registration below. */
        let (pv, uv) = match ruby_try_verified_text_pair(ks.as_value(), vs.as_value(), cap) {
            Ok(pair) => pair,
            Err(reason) => {
                return Err(Error::new(
                    error_class(),
                    format!("invalid namespace mapping: {}", reason.to_string_lossy()),
                ));
            }
        };
        // SAFETY: both views are live and checked; `register_ns` copies both.
        let registered =
            ctx.register_ns(pv.as_verified().as_bytes(), uv.as_verified().as_bytes());
        if registered.is_err() {
            return Err(Error::new(error_class(), "failed to register namespace"));
        }
    }
    Ok(())
}

/// Build the query context every XML XPath/CSS entry point runs under, rooted at
/// `context` (a node, or a Document for its document node), with `rb_ns`
/// registered for this query alone.
///
/// The query text's contract is verified FIRST, before the context exists. The
/// borrowed view the parse reads is minted later, by `parse_query`, so its bytes
/// are never held across the GC points namespace registration goes through.
fn build_ctx(
    ruby: &Ruby,
    context: Value,
    document: Value,
    rb_text: Value,
    what: &core::ffi::CStr,
    rb_ns: Option<Value>,
) -> Result<XPathContext, Error> {
    verify_text(crate::bridge::ruby::string_of(rb_text)?.as_value(), what)?;
    let ctx = context_for(context, document)?;
    register_namespaces(ruby, &ctx, rb_ns)?; /* ctx drops on error */
    Ok(ctx)
}

/// Evaluate a compiled AST with no handler and convert the result, freeing the
/// AST and the context first.
fn run_ast(
    ruby: &Ruby,
    ctx: XPathContext,
    ast: Box<Ast>,
    first_only: bool,
    document: Value,
) -> Result<Value, Error> {
    let nil = ruby.qnil().as_value();
    let value = evaluate_query(&ctx, &ast, nil, document, first_only);
    drop(ast);
    drop(ctx);
    query_result(value?, document, first_only)
}

/// `#xpath(expr, namespaces = nil)` / `#at_xpath(...)`.
///
/// Evaluated over the XML engine instance, rooted at `self`'s context node.
/// `namespaces` is registered for this query alone - a default-namespace
/// document (RSS, Atom) needs a prefix under strict matching.
/// `Makiri::XPathContext` is the alternative when many queries share one
/// namespace set, since it caches both the registrations and the compiled ASTs.
fn xpath_run(
    ruby: &Ruby,
    rb_self: Value,
    expr: Value,
    ns: Option<Value>,
    first_only: bool,
) -> Result<Value, Error> {
    let (document, context) = query_context(rb_self)?;
    if context.is_invalid() {
        return Ok(if first_only {
            ruby.qnil().as_value()
        } else {
            node_set_new(document)
        });
    }
    let ctx = build_ctx(ruby, rb_self, document, expr, c"XPath expression", ns)?;
    /* Parse AFTER namespace registration: that step allocates Ruby objects and
     * may run a GC, and the borrowed expression bytes must not be live across
     * one. */
    let ast = parse_query(&ctx, expr)?;
    run_ast(ruby, ctx, ast, first_only, document)
}

fn xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    xpath_run(ruby, rb_self, a.required.0, a.optional.0, false)
}

fn at_xpath(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    xpath_run(ruby, rb_self, a.required.0, a.optional.0, true)
}

/* ---- CSS over XML, lowered to the native XPath engine ----
 *
 * A selector is compiled to the engine's AST and run through the SAME evaluator
 * as #xpath, so case-sensitivity, namespaces, budgets and document order are
 * identical. The Ruby wrappers collect the document's namespaces and pass a
 * normalised {prefix => uri} hash; a default namespace arrives under the
 * synthetic "xmlns" prefix, which a bare type selector binds to. */

/// Whether the (already prefix-normalised) namespace hash carries "xmlns".
fn css_default_namespace(rb_ns: Option<Value>) -> bool {
    let Some(h) = rb_ns.and_then(RHash::from_value) else {
        return false;
    };
    let ruby = Ruby::get_with(h);
    matches!(h.get(ruby.str_new(CSS_DEFAULT_NS_PREFIX)), Some(found) if !found.is_nil())
}

/// Compile a selector under `ctx`, whose namespaces are already registered.
fn css_compile_or_raise(
    ctx: &XPathContext,
    selector: Value,
    rb_ns: Option<Value>,
) -> Result<Box<Ast>, Error> {
    let cns = CssNs {
        default_namespace: css_default_namespace(rb_ns),
    };
    let sv = ruby_verified_text(selector, c"CSS selector")?;
    let mut budget = crate::xpath::limits::Budget::with_limits(ctx.limits());
    /* `sv` holds the selector String rooted; the compile allocates through
     * falloc only - no Ruby runs in it. */
    let ast = crate::css::compile_owned(sv.as_verified(), &cns, &mut budget);
    drop(sv);
    if let Ok(ast) = ast {
        return Ok(ast);
    }
    let error = budget.take_error();

    if error.status == XP_ERR_SYNTAX {
        let msg = error.message().map_or_else(
            || "invalid CSS selector".to_string(),
            |m| m.to_string_lossy().into_owned(),
        );
        return Err(Error::new(EXC_CSS_SYNTAX_ERROR.exception(), msg));
    }
    Err(xpath_error(&error))
}

fn css_run(
    ruby: &Ruby,
    rb_self: Value,
    selector: Value,
    ns: Value,
    first_only: bool,
) -> Result<Value, Error> {
    let (document, context) = query_context(rb_self)?;
    if context.is_invalid() {
        return Ok(if first_only {
            ruby.qnil().as_value()
        } else {
            node_set_new(document)
        });
    }
    let ctx = build_ctx(ruby, rb_self, document, selector, c"CSS selector", Some(ns))?;
    let ast = css_compile_or_raise(&ctx, selector, Some(ns))?;
    run_ast(ruby, ctx, ast, first_only, document)
}

fn css(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<Value, Error> {
    css_run(ruby, rb_self, selector, ns, false)
}

fn at_css(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<Value, Error> {
    css_run(ruby, rb_self, selector, ns, true)
}

/// `#matches?(selector)`: does THIS node match?
///
/// Evaluated by selecting every match in the whole document - the context is the
/// document node, so a descendant-rooted selector scans the entire tree - and
/// testing membership by node identity. That is the semantics that stays correct
/// with every combinator.
fn css_matches(ruby: &Ruby, rb_self: Value, selector: Value, ns: Value) -> Result<bool, Error> {
    let (document, node) = query_context(rb_self)?;
    if node.is_invalid() {
        return Ok(false);
    }
    /* Rooted at the document node: see above. */
    let ctx = build_ctx(
        ruby,
        document,
        document,
        selector,
        c"CSS selector",
        Some(ns),
    )?;
    let ast = css_compile_or_raise(&ctx, selector, Some(ns))?;

    let nil = ruby.qnil().as_value();
    let value = evaluate_query(&ctx, &ast, nil, document, false);
    drop(ast);
    let value = value?;
    let target = crate::xpath::token::Token::from_ptr(node.to_token() as *mut c_void);
    Ok(matches!(&value, XPathValue::NodeSet(set) if set.as_slice().contains(&target)))
}

/* ------------------------------------------------------------------ */
/* documents and fragments                                            */
/* ------------------------------------------------------------------ */

fn doc_root(ruby: &Ruby, rb_self: Value) -> Value {
    crate::bridge::xml::document_root(ruby, rb_self)
}

/// The document's DOCTYPE, or nil.
///
/// The name and the external/system identifiers are read; the DTD body is NOT
/// parsed - no entity or element declarations are loaded, so `&name;` stays an
/// undefined-entity error and no external subset is fetched. The node is kept
/// off the tree, so XPath never sees it (XPath 1.0 has no doctype node type).
fn doc_internal_subset(ruby: &Ruby, rb_self: Value) -> Value {
    crate::bridge::xml::document_internal_subset(ruby, rb_self)
}

/// `Makiri::XML::Document.new` - an empty document to build up programmatically.
/// Any arguments (Nokogiri accepts a version and encoding) are accepted and
/// ignored.
fn document_s_new(_args: &[Value]) -> Result<Value, Error> {
    crate::bridge::xml::new_empty_xml_document()
}

/// `Makiri::XML::DocumentFragment.parse(source)` - a standalone fragment with
/// its own empty backing document. Self-contained: a prefixed name must declare
/// its namespace within the fragment itself. Use `Document#fragment` to parse
/// against an existing document's in-scope namespaces instead.
fn fragment_s_parse(_klass: Value, source: Value) -> Result<Value, Error> {
    let doc_obj = crate::bridge::xml::new_empty_xml_document()?;
    let frag = crate::bridge::xml::fragment_into(doc_obj, source, false)?;
    Ok(wrap_typed_xml_node(frag, doc_obj))
}

/// `doc.fragment(source)` - a fragment bound to this document, resolving names
/// against its in-scope (root) namespaces, so the nodes can be spliced in.
fn doc_fragment(rb_self: Value, source: Value) -> Result<Value, Error> {
    let frag = crate::bridge::xml::fragment_into(rb_self, source, true)?;
    Ok(wrap_typed_xml_node(frag, rb_self))
}

/// # Safety
/// Called from `Init_makiri`.
pub fn init_xml() {
    let ruby = Ruby::get().expect("init runs on the Ruby thread");
    let m_xml = magnus::RModule::from_value(MOD_XML.value()).expect("Makiri::XML");
    let base = magnus::RClass::from_value(CLASS_DOCUMENT.value()).expect("Makiri::Document");

    /* XML::Document is a Makiri::Document leaf: is_a?(Makiri::Document) holds,
     * but it carries no HTML readers - those live on Makiri::HTML, which it does
     * not include. The read-only XML surface is structural. */
    let doc = m_xml
        .define_class("Document", base)
        .expect("Makiri::XML::Document");
    doc.undef_default_alloc_func(); /* created only from C, never .new */
    let node_methods =
        magnus::RModule::from_value(MOD_XML_NODE_METHODS.value()).expect("NodeMethods");
    doc.include_module(node_methods)
        .expect("include NodeMethods");
    /* Init_makiri's global, which the rest of the extension reads. */
    crate::init::record_xml_document_class(doc);

    doc.define_method("root", method!(doc_root, 0))
        .expect("#root");
    doc.define_method("internal_subset", method!(doc_internal_subset, 0))
        .expect("#internal_subset");
    doc.define_method("fragment", method!(doc_fragment, 1))
        .expect("#fragment");
    doc.define_singleton_method("new", magnus::function!(document_s_new, -1))
        .expect("Document.new");
    magnus::RClass::from_value(CLASS_XML_DOCUMENT_FRAGMENT.value())
        .expect("XML::DocumentFragment")
        .define_singleton_method("parse", method!(fragment_s_parse, 1))
        .expect("DocumentFragment.parse");

    /* xpath / at_xpath work on the document and on any XML node (rooted there),
     * so they go on the shared node-behaviour module as well as the document. */
    doc.define_method("xpath", method!(xpath, -1))
        .expect("Document#xpath");
    doc.define_method("at_xpath", method!(at_xpath, -1))
        .expect("Document#at_xpath");
    node_methods
        .define_method("xpath", method!(xpath, -1))
        .expect("Node#xpath");
    node_methods
        .define_method("at_xpath", method!(at_xpath, -1))
        .expect("Node#at_xpath");

    /* CSS over XML: the private primitives the Ruby #css / #at_css / #matches?
     * wrappers call once they have collected the document's namespaces. */
    node_methods
        .define_private_method("_css", method!(css, 2))
        .expect("#_css");
    node_methods
        .define_private_method("_at_css", method!(at_css, 2))
        .expect("#_at_css");
    node_methods
        .define_private_method("_css_matches", method!(css_matches, 2))
        .expect("#_css_matches");

    /* The native parser, mirroring HTML::Document.parse. Makiri::XML(source)
     * delegates to it in Ruby. */
    doc.define_singleton_method("parse", magnus::function!(s_parse, -1))
        .expect("Document.parse");
    let _ = ruby;
}
