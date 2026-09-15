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

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RArray, RHash, RString, Ruby, Value};
use rb_sys::VALUE;

use crate::xml::model::{Doc as XmlDoc, Limits as XmlLimits, NodeId};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::XPathValue;
use crate::xpath::msg::XP_ERR_SYNTAX;

use super::abi::error_class;

/* The statuses and the arena ceiling come from `crate::xml::model` rather than
 * being restated here: that module is the XML engine's own declaration of them,
 * and the statuses are now a real enum, so the compiler holds the two copies
 * together. */
use crate::xml::model::{Status, MAX_BYTES};

/// `MKR_CSS_DEFAULT_NS_PREFIX` - the synthetic prefix a default namespace
/// arrives under, Nokogiri's convention, so a bare type selector binds to it.
const CSS_DEFAULT_NS_PREFIX: &str = "xmlns";

/// The engine context. Opaque here while C held it; now the real type.
use crate::xpath::ctx::Context as XPathContext;

/// The default-namespace prefix, or NULL. Declared twice while C held it (once
/// here, once in `css`); the fields matched, but nothing checked that.
use crate::css::CssNs;

use super::abi::{
    doc_parsed, keepalive_document, node_set_new, parsed_xml_doc, ruby_verified_text, verify_text,
    wrap_xml_node, xml_node_unwrap, OwnedBytes, CLASS_DOCUMENT, CLASS_XML_DOCUMENT,
    CLASS_XML_DOCUMENT_FRAGMENT, EXC_CSS_SYNTAX_ERROR, EXC_ERROR, EXC_XML_LIMIT_EXCEEDED,
    EXC_XML_SYNTAX_ERROR, MOD_XML, MOD_XML_NODE_METHODS,
};

/// Wrap an XML node, typed.
unsafe fn wrap_typed_xml_node(node: NodeId, document: VALUE) -> VALUE {
    wrap_xml_node(node.to_token() as *mut c_void, document)
}

/// The XML node behind a wrapper, typed. `Err(TypeError)` for an HTML node.
unsafe fn typed_xml_node_unwrap(rb_node: VALUE) -> Result<NodeId, Error> {
    Ok(NodeId::from_token(xml_node_unwrap(rb_node)? as usize))
}

pub use crate::bridge::string::ruby_copy_bytes;
pub use crate::bridge::string::ruby_try_verified_text;
pub use crate::bridge::xml_decode::xml_decode_input;
use crate::dom_adapter::post_parse::Parsed;
pub use crate::glue::doc::wrap_document;
use crate::glue::xpath::xpath_error;
use crate::glue::xpath::{context_for, evaluate_query, parse_query, query_result};
pub use crate::xml::api::xml_doc_new;
pub use crate::xml::api::xml_parse_ex;
pub use crate::xml::api::xml_parse_fragment;

extern "C" {

    fn rb_thread_call_without_gvl(
        func: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        data: *mut c_void,
        ubf: *const c_void,
        ubf_data: *mut c_void,
    ) -> *mut c_void;
}

/* ------------------------------------------------------------------ */
/* parse                                                              */
/* ------------------------------------------------------------------ */

/// What crosses into the GVL-released closure: plain data only. No `VALUE`, no
/// `Ruby` handle - that rule is what makes the release safe.
struct ParseWork {
    src: *const c_char,
    len: usize,
    limits: XmlLimits,
    result: *mut XmlDoc,
    status: Status,
}

unsafe extern "C" fn parse_nogvl(arg: *mut c_void) -> *mut c_void {
    let w = &mut *(arg as *mut ParseWork);
    let src = if w.src.is_null() || w.len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(w.src as *const u8, w.len)
    };
    match xml_parse_ex(src, Some(&w.limits)) {
        Ok(doc) => {
            w.result = Box::into_raw(doc);
            w.status = Status::Ok;
        }
        Err(status) => {
            w.result = core::ptr::null_mut();
            w.status = status;
        }
    }
    core::ptr::null_mut()
}

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

    unsafe {
        /* Strict decode under the GVL: invalid UTF-8, an undecodable byte or a
         * NUL all raise here, with no U+FFFD repair. The budget goes in so an
         * over-large input is refused before its validation copy AND before the
         * copy below - a hostile document is never materialised twice for a
         * parse that cannot succeed. */
        let decoded = xml_decode_input(rb_sys::rb_String(source.as_raw()), budget);

        /* Copy into a private buffer BEFORE allocating any Ruby object, so there
         * is no GC point between obtaining `decoded` and copying it. */
        let mut src = match ruby_copy_bytes(decoded) {
            Some(src) => src,
            None => {
                return Err(Error::new(
                    error_class(),
                    "out of memory copying XML source",
                ))
            }
        };

        /* Wrap an empty handle first, so a failure mid-parse still frees
         * cleanly through the GC. The source is already copied, so this Ruby
         * allocation cannot disturb it. */
        let Some(parsed) = Parsed::new_xml() else {
            free_owned(&mut src);
            return Err(Error::new(
                error_class(),
                "out of memory allocating XML document",
            ));
        };
        let parsed = Box::into_raw(parsed);
        let obj = wrap_document(parsed); /* GC owns `parsed` from here */

        let mut work = ParseWork {
            src: src.ptr,
            len: src.len,
            limits,
            result: core::ptr::null_mut(),
            status: Status::Ok,
        };
        rb_thread_call_without_gvl(
            parse_nogvl,
            &mut work as *mut ParseWork as *mut c_void,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        free_owned(&mut src);

        if work.result.is_null() {
            return Err(parse_status_error(work.status, Unit::Document));
        }
        (*parsed).set_xml_doc(Box::from_raw(work.result));
        Ok(Value::from_raw(obj))
    }
}

unsafe fn free_owned(b: &mut OwnedBytes) {
    if !b.ptr.is_null() {
        libc_free(b.ptr as *mut c_void);
        b.ptr = core::ptr::null_mut();
        b.len = 0;
    }
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

/// Which entry point failed. The two carry their own wording rather than one
/// composed string: the document path says "malformed XML" where the fragment
/// path says "malformed XML fragment", and the messages are observable.
#[derive(Clone, Copy)]
enum Unit {
    Document,
    Fragment,
}

impl Unit {
    fn malformed(self) -> &'static str {
        match self {
            Unit::Document => "malformed XML",
            Unit::Fragment => "malformed XML fragment",
        }
    }
    fn budget(self) -> &'static str {
        match self {
            Unit::Document => "XML document budget exceeded",
            Unit::Fragment => "XML fragment budget exceeded",
        }
    }
    fn failed(self) -> &'static str {
        match self {
            Unit::Document => "failed to parse XML document",
            Unit::Fragment => "failed to parse XML fragment",
        }
    }
}

/// Map a parse status onto its Ruby exception.
unsafe fn parse_status_error(status: Status, unit: Unit) -> Error {
    let class = |v: VALUE| {
        magnus::ExceptionClass::from_value(Value::from_raw(v)).expect("an exception class")
    };
    match status {
        Status::Syntax => Error::new(class(EXC_XML_SYNTAX_ERROR.raw()), unit.malformed()),
        Status::Limit => Error::new(class(EXC_XML_LIMIT_EXCEEDED.raw()), unit.budget()),
        Status::Version => Error::new(
            class(EXC_XML_SYNTAX_ERROR.raw()),
            "unsupported XML version (only XML 1.0 is supported)",
        ),
        /* `Ok` never reaches here (it means no failure); the rest are the
         * generic "failed to parse" bucket. */
        Status::Ok | Status::Oom | Status::Internal => {
            Error::new(class(EXC_ERROR.raw()), unit.failed())
        }
    }
}

/* ------------------------------------------------------------------ */
/* queries                                                            */
/* ------------------------------------------------------------------ */

/// The (Document VALUE, context node) a query runs against: for a Document the
/// context is the arena's document node, for a node it is that node.
unsafe fn query_context(rb_self: Value) -> Result<(Value, NodeId), Error> {
    /* `xml_node_unwrap` is kind-checked - `Err` for a non-XML node - and
     * resolves an XML Document to its document node. */
    let document = Value::from_raw(keepalive_document(rb_self.as_raw())?);
    Ok((document, typed_xml_node_unwrap(rb_self.as_raw())?))
}

/// Register a `{prefix => uri}` Hash onto `ctx` for one query.
///
/// On any bad entry an error is returned, and the caller's owner frees the
/// context - never a partial registration. RSS and Atom live in a default
/// namespace, so a prefix is the strict-mode way to select them.
unsafe fn register_namespaces(
    ruby: &Ruby,
    ctx: &XPathContext,
    rb_ns: Option<Value>,
) -> Result<(), Error> {
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

        let pair = ruby_try_verified_text(ks.as_raw(), cap)
            .and_then(|pv| Ok((pv, ruby_try_verified_text(vs.as_raw(), cap)?)));
        let (pv, uv) = match pair {
            Ok(pair) => pair,
            Err(reason) => {
                return Err(Error::new(
                    error_class(),
                    format!("invalid namespace mapping: {}", reason.to_string_lossy()),
                ));
            }
        };
        if ctx
            .register_ns(pv.as_verified().as_bytes(), uv.as_verified().as_bytes())
            .is_err()
        {
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
unsafe fn build_ctx(
    ruby: &Ruby,
    context: Value,
    document: Value,
    rb_text: Value,
    what: *const c_char,
    rb_ns: Option<Value>,
) -> Result<XPathContext<'static>, Error> {
    verify_text(crate::bridge::ruby::string_of(rb_text)?.as_raw(), what)?;
    let ctx = context_for(context, document)?;
    register_namespaces(ruby, &ctx, rb_ns)?; /* ctx drops on error */
    Ok(ctx)
}

/// Evaluate a compiled AST with no handler and convert the result, freeing the
/// AST and the context first.
unsafe fn run_ast(
    ruby: &Ruby,
    ctx: XPathContext<'static>,
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
    unsafe {
        let (document, context) = query_context(rb_self)?;
        if context.is_invalid() {
            return Ok(if first_only {
                ruby.qnil().as_value()
            } else {
                Value::from_raw(node_set_new(document.as_raw()))
            });
        }
        let ctx = build_ctx(
            ruby,
            rb_self,
            document,
            expr,
            c"XPath expression".as_ptr(),
            ns,
        )?;
        /* Parse AFTER namespace registration: that step allocates Ruby objects
         * and may run a GC, and the borrowed expression bytes must not be live
         * across one. */
        let ast = parse_query(&ctx, expr)?;
        run_ast(ruby, ctx, ast, first_only, document)
    }
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
unsafe fn css_default_namespace(rb_ns: Option<Value>) -> bool {
    let Some(h) = rb_ns.and_then(RHash::from_value) else {
        return false;
    };
    let ruby = Ruby::get_unchecked();
    matches!(h.get(ruby.str_new(CSS_DEFAULT_NS_PREFIX)), Some(found) if !found.is_nil())
}

/// Compile a selector under `ctx`, whose namespaces are already registered.
unsafe fn css_compile_or_raise(
    ctx: &XPathContext,
    selector: Value,
    rb_ns: Option<Value>,
) -> Result<Box<Ast>, Error> {
    let cns = CssNs {
        default_namespace: css_default_namespace(rb_ns),
    };
    let sv = ruby_verified_text(selector.as_raw(), c"CSS selector".as_ptr())?;
    let mut budget = crate::xpath::limits::Budget::with_limits(ctx.limits());
    let ast = crate::css::compile_owned(unsafe { sv.as_verified() }, &cns, &mut budget);
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
        let class = magnus::ExceptionClass::from_value(EXC_CSS_SYNTAX_ERROR.value())
            .expect("Makiri::CSS::SyntaxError");
        return Err(Error::new(class, msg));
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
    unsafe {
        let (document, context) = query_context(rb_self)?;
        if context.is_invalid() {
            return Ok(if first_only {
                ruby.qnil().as_value()
            } else {
                Value::from_raw(node_set_new(document.as_raw()))
            });
        }
        let ctx = build_ctx(
            ruby,
            rb_self,
            document,
            selector,
            c"CSS selector".as_ptr(),
            Some(ns),
        )?;
        let ast = css_compile_or_raise(&ctx, selector, Some(ns))?;
        run_ast(ruby, ctx, ast, first_only, document)
    }
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
    unsafe {
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
            c"CSS selector".as_ptr(),
            Some(ns),
        )?;
        let ast = css_compile_or_raise(&ctx, selector, Some(ns))?;

        let nil = ruby.qnil().as_value();
        let value = evaluate_query(&ctx, &ast, nil, document, false);
        drop(ast);
        let value = value?;
        let target = node.to_token() as *mut c_void;
        Ok(matches!(&value, XPathValue::NodeSet(set) if set.as_slice().contains(&target)))
    }
}

/* ------------------------------------------------------------------ */
/* documents and fragments                                            */
/* ------------------------------------------------------------------ */

fn doc_root(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let xdoc = parsed_xml_doc(crate::glue::doc::doc_parsed_known(rb_self.as_raw()));
        if xdoc.is_null() {
            return ruby.qnil().as_value();
        }
        Value::from_raw(wrap_typed_xml_node(
            (*xdoc).root.unwrap_or(NodeId::INVALID),
            rb_self.as_raw(),
        ))
    }
}

/// The document's DOCTYPE, or nil.
///
/// The name and the external/system identifiers are read; the DTD body is NOT
/// parsed - no entity or element declarations are loaded, so `&name;` stays an
/// undefined-entity error and no external subset is fetched. The node is kept
/// off the tree, so XPath never sees it (XPath 1.0 has no doctype node type).
fn doc_internal_subset(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let xdoc = parsed_xml_doc(crate::glue::doc::doc_parsed_known(rb_self.as_raw()));
        if xdoc.is_null() || (*xdoc).doctype.is_none() {
            return ruby.qnil().as_value();
        }
        Value::from_raw(wrap_typed_xml_node(
            (*xdoc).doctype.unwrap_or(NodeId::INVALID),
            rb_self.as_raw(),
        ))
    }
}

/// Strict-decode `source` and parse it as a fragment into `xdoc`.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
unsafe fn fragment_into(
    xdoc: *mut XmlDoc,
    source: Value,
    inherit_doc_ns: bool,
) -> Result<NodeId, Error> {
    let decoded = xml_decode_input(rb_sys::rb_String(source.as_raw()), (*xdoc).max_bytes);
    let mut src = match ruby_copy_bytes(decoded) {
        Some(src) => src,
        None => {
            return Err(Error::new(
                error_class(),
                "out of memory copying XML fragment source",
            ))
        }
    };
    let bytes = if src.ptr.is_null() || src.len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src.ptr as *const u8, src.len)
    };
    let frag = xml_parse_fragment(&mut *xdoc, bytes, inherit_doc_ns);
    free_owned(&mut src);
    frag.map_err(|status| parse_status_error(status, Unit::Fragment))
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
unsafe fn new_empty_document() -> Result<Value, Error> {
    let Some(parsed) = Parsed::new_xml() else {
        return Err(Error::new(
            error_class(),
            "out of memory allocating XML document",
        ));
    };
    let parsed = Box::into_raw(parsed);
    let doc_obj = wrap_document(parsed); /* GC owns `parsed` from here */
    let xdoc = match xml_doc_new() {
        Ok(doc) => doc,
        Err(_) => {
            return Err(Error::new(
                error_class(),
                "out of memory allocating XML document",
            ));
        }
    };
    (*parsed).set_xml_doc(xdoc); /* GC now frees `xdoc` via `parsed` */
    Ok(Value::from_raw(doc_obj))
}

/// `Makiri::XML::Document.new` - an empty document to build up programmatically.
/// Any arguments (Nokogiri accepts a version and encoding) are accepted and
/// ignored.
fn document_s_new(_args: &[Value]) -> Result<Value, Error> {
    unsafe { new_empty_document() }
}

/// `Makiri::XML::DocumentFragment.parse(source)` - a standalone fragment with
/// its own empty backing document. Self-contained: a prefixed name must declare
/// its namespace within the fragment itself. Use `Document#fragment` to parse
/// against an existing document's in-scope namespaces instead.
fn fragment_s_parse(_klass: Value, source: Value) -> Result<Value, Error> {
    unsafe {
        let doc_obj = new_empty_document()?;
        let xdoc = parsed_xml_doc(doc_parsed(doc_obj.as_raw())?);
        let frag = fragment_into(xdoc, source, false)?;
        Ok(Value::from_raw(wrap_typed_xml_node(frag, doc_obj.as_raw())))
    }
}

/// `doc.fragment(source)` - a fragment bound to this document, resolving names
/// against its in-scope (root) namespaces, so the nodes can be spliced in.
fn doc_fragment(rb_self: Value, source: Value) -> Result<Value, Error> {
    unsafe {
        let xdoc = parsed_xml_doc(doc_parsed(rb_self.as_raw())?);
        if xdoc.is_null() {
            return Err(Error::new(error_class(), "the document has no arena"));
        }
        let frag = fragment_into(xdoc, source, true)?;
        Ok(Value::from_raw(wrap_typed_xml_node(frag, rb_self.as_raw())))
    }
}

/// # Safety
/// Called from `Init_makiri`.
pub unsafe extern "C" fn init_xml() {
    let ruby = Ruby::get_unchecked();
    let m_xml = magnus::RModule::from_value(MOD_XML.value()).expect("Makiri::XML");
    let base = magnus::RClass::from_value(CLASS_DOCUMENT.value()).expect("Makiri::Document");

    /* XML::Document is a Makiri::Document leaf: is_a?(Makiri::Document) holds,
     * but it carries no HTML readers - those live on Makiri::HTML, which it does
     * not include. The read-only XML surface is structural. */
    let doc = m_xml
        .define_class("Document", base)
        .expect("Makiri::XML::Document");
    rb_sys::rb_undef_alloc_func(doc.as_raw()); /* created only from C, never .new */
    let node_methods =
        magnus::RModule::from_value(MOD_XML_NODE_METHODS.value()).expect("NodeMethods");
    doc.include_module(node_methods)
        .expect("include NodeMethods");
    /* Init_makiri's global, which the rest of the extension reads. */
    CLASS_XML_DOCUMENT.set(doc.as_raw());

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
