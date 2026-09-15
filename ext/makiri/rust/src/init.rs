//! `Init_makiri` - the registration seam (makiri.c).
//!
//! Ruby loads the extension by calling `Init_makiri`, and this is it:
//! `#[magnus::init]` generates a symbol with exactly that name and the C ABI, so
//! the contract "export only `Init_makiri`" is unchanged - it is enforced by the
//! linker flag, as it always was, not by the language the function is written in.
//!
//! # Why the class VALUEs are `static mut`
//!
//! Forty-six classes and modules are created here and read from a dozen other
//! modules. In C they were plain globals; they are the same object with the same
//! contract here. Thirty-five are exported because something outside this file
//! reads them; the other eleven exist only long enough to build the hierarchy
//! and stay local.
//!
//! They are written exactly once, during `init`, before any Ruby code can run,
//! and only read afterwards. That is what makes the `static mut` sound, and it
//! is the same argument the C relied on - not a weaker one.
//!
//! # What the hierarchy encodes
//!
//! `Makiri::Node` and its kind classes are ABSTRACT bases. The concrete nodes
//! are `Makiri::HTML::*` and `Makiri::XML::*` leaves, and the reader methods
//! live on a per-representation behaviour module (`HTML::NodeMethods`,
//! `XML::NodeMethods`) included into each leaf. That is what stops an XML node
//! from ever inheriting a reader that dereferences a Lexbor node: the two
//! representations share a base class but never a method.
//!
//! Every leaf also loses its allocator. These objects are created only from
//! Rust, wrapping a live node; `.new` would hand back one wrapping nothing.

#![allow(non_upper_case_globals)]

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{function, Error, Module, Object, Ruby, Value};
use rb_sys::VALUE;

/* ------------------------------------------------------------------ *
 * the classes and modules other modules read                         *
 * ------------------------------------------------------------------ *
 *
 * Exported under the same names the C used, so every `mkr_init_*` and every
 * glue module that already reads one keeps working unchanged. */

macro_rules! exported {
    ($($name:ident),* $(,)?) => {
        $(
            pub static mut $name: VALUE = 0;
        )*
    };
}

exported! {
    mkr_cNode, mkr_cDocument, mkr_cDocumentFragment, mkr_cNodeSet, mkr_cXPathContext,
    mkr_mXML, mkr_mLexbor,
    mkr_mHtmlNodeMethods, mkr_cHtmlNode, mkr_cHtmlDocument, mkr_cHtmlElement,
    mkr_cHtmlAttr, mkr_cHtmlText, mkr_cHtmlComment, mkr_cHtmlCDATASection,
    mkr_cHtmlProcessingInstruction, mkr_cHtmlDocumentType, mkr_cHtmlDocumentFragment,
    mkr_mXmlNodeMethods, mkr_cXmlNode, mkr_cXmlDocument, mkr_cXmlElement,
    mkr_cXmlAttr, mkr_cXmlText, mkr_cXmlComment, mkr_cXmlCDATASection,
    mkr_cXmlProcessingInstruction, mkr_cXmlDocumentType, mkr_cXmlDocumentFragment,
    mkr_eError, mkr_eXPathSyntaxError, mkr_eXPathLimitExceeded,
    mkr_eCSSSyntaxError, mkr_eXmlSyntaxError, mkr_eXmlLimitExceeded,
}

/* ------------------------------------------------------------------ *
 * the test hooks                                                     *
 * ------------------------------------------------------------------ */

/// Whether this build carries the allocation-failure hook.
///
/// False in a normal build, so `rake oom`'s harness fails loudly on the wrong
/// build instead of sweeping nothing.
fn alloc_inject_p() -> bool {
    cfg!(feature = "alloc-inject")
}

/// `Makiri.__alloc_inject(n)` - arm "the nth core allocation fails once".
fn alloc_inject(ruby: &Ruby, nth: i64) -> Result<(), Error> {
    #[cfg(feature = "alloc-inject")]
    {
        unsafe { crate::falloc::calloc::alloc_inject_arm(nth) };
        let _ = ruby;
        Ok(())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        let _ = nth;
        Err(Error::new(
            ruby.exception_not_imp_error(),
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__alloc_inject_calls` - how many core allocations a workload
/// attempted, armed or not, so the sweep can size itself from a disarmed run.
fn alloc_inject_calls(ruby: &Ruby) -> Result<u64, Error> {
    #[cfg(feature = "alloc-inject")]
    {
        let _ = ruby;
        Ok(unsafe { crate::falloc::calloc::alloc_inject_call_count() })
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        Err(Error::new(
            ruby.exception_not_imp_error(),
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri::XML.__decode(str)` - the strict input decode in isolation, without
/// the tokenizer or the tree builder (`spec/xml_decode_spec.rb`).
fn xml_decode(ruby: &Ruby, str: Value) -> Value {
    let _ = ruby;
    /* decode-only: no arena, no budget */
    unsafe {
        Value::from_raw(crate::bridge::xml_decode::xml_decode_input(
            rb_sys::rb_String(str.as_raw()),
            0,
        ))
    }
}

/* ------------------------------------------------------------------ *
 * Init_makiri                                                        *
 * ------------------------------------------------------------------ */

/// Give every leaf the representation's reader module and take away its
/// allocator.
///
/// The two go together: a leaf carries the readers because it wraps a live
/// node, and loses `.new` for the same reason.
unsafe fn seal_leaves(methods: VALUE, leaves: &[VALUE]) {
    for &leaf in leaves {
        rb_sys::rb_include_module(leaf, methods);
        rb_sys::rb_undef_alloc_func(leaf);
    }
}

/// `name = "makiri"` is load-bearing: the attribute defaults to the CRATE name,
/// which would export `Init_makiri_rs`. Ruby looks up `Init_<basename of the
/// .bundle>`, so the default would leave the extension loadable-looking and
/// unloadable - and the "export only `Init_makiri`" check would pass, having
/// found the wrong symbol.
#[magnus::init(name = "makiri")]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let makiri = ruby.define_module("Makiri")?;

    /* The abstract bases. Concrete nodes are the HTML::* / XML::* leaves
     * below; these exist so `is_a?(Makiri::Element)` holds across both. */
    let node = makiri.define_class("Node", ruby.class_object())?;
    let document = makiri.define_class("Document", node)?;
    let element = makiri.define_class("Element", node)?;
    let attr = makiri.define_class("Attr", node)?;
    let text = makiri.define_class("Text", node)?;
    let comment = makiri.define_class("Comment", node)?;
    let cdata = makiri.define_class("CDATASection", node)?;
    let pi = makiri.define_class("ProcessingInstruction", node)?;
    let doctype = makiri.define_class("DocumentType", node)?;
    let fragment = makiri.define_class("DocumentFragment", node)?;
    let node_set = makiri.define_class("NodeSet", ruby.class_object())?;
    let xpath_context = makiri.define_class("XPathContext", ruby.class_object())?;

    let m_xpath = makiri.define_module("XPath")?;
    let m_css = makiri.define_module("CSS")?;
    let m_xml = makiri.define_module("XML")?;
    let m_lexbor = makiri.define_module("Lexbor")?;

    /* Makiri::HTML - the Lexbor-backed leaves. */
    let m_html = makiri.define_module("HTML")?;
    let html_methods = m_html.define_module("NodeMethods")?;
    let h_node = m_html.define_class("Node", node)?;
    let h_document = m_html.define_class("Document", document)?;
    let h_element = m_html.define_class("Element", element)?;
    let h_attr = m_html.define_class("Attr", attr)?;
    let h_text = m_html.define_class("Text", text)?;
    let h_comment = m_html.define_class("Comment", comment)?;
    let h_cdata = m_html.define_class("CDATASection", cdata)?;
    let h_pi = m_html.define_class("ProcessingInstruction", pi)?;
    let h_doctype = m_html.define_class("DocumentType", doctype)?;
    let h_fragment = m_html.define_class("DocumentFragment", fragment)?;

    /* Makiri::XML - the arena-backed leaves. XML::Document is defined by
     * mkr_init_xml, because it backs a parse handle rather than a node. */
    let xml_methods = m_xml.define_module("NodeMethods")?;
    let x_node = m_xml.define_class("Node", node)?;
    let x_element = m_xml.define_class("Element", element)?;
    let x_attr = m_xml.define_class("Attr", attr)?;
    let x_text = m_xml.define_class("Text", text)?;
    let x_comment = m_xml.define_class("Comment", comment)?;
    let x_cdata = m_xml.define_class("CDATASection", cdata)?;
    let x_pi = m_xml.define_class("ProcessingInstruction", pi)?;
    /* DocumentType descends from the SHARED base, not XML::Node, so
     * `is_a?(Makiri::DocumentType)` holds for both representations. It is still
     * an XML leaf: the readers come from the module included below. */
    let x_doctype = m_xml.define_class("DocumentType", doctype)?;
    let x_fragment = m_xml.define_class("DocumentFragment", fragment)?;

    /* `define_error`, not `define_class`: an exception class is an
     * `ExceptionClass` in magnus, and raising through one is the only thing
     * these are for. LimitExceeded descends from XPath::SyntaxError (so
     * rescuing the syntax error catches the budget too), while its XML
     * counterpart descends from Error - that asymmetry is deliberate and
     * predates the port. */
    let err = makiri.define_error("Error", ruby.exception_standard_error())?;
    let xpath_syntax = m_xpath.define_error("SyntaxError", err)?;
    let xpath_limit = m_xpath.define_error("LimitExceeded", xpath_syntax)?;
    let css_syntax = m_css.define_error("SyntaxError", err)?;
    let xml_syntax = m_xml.define_error("SyntaxError", err)?;
    let xml_limit = m_xml.define_error("LimitExceeded", err)?;

    unsafe {
        mkr_cNode = node.as_raw();
        mkr_cDocument = document.as_raw();
        mkr_cDocumentFragment = fragment.as_raw();
        mkr_cNodeSet = node_set.as_raw();
        mkr_cXPathContext = xpath_context.as_raw();
        mkr_mXML = m_xml.as_raw();
        mkr_mLexbor = m_lexbor.as_raw();

        mkr_mHtmlNodeMethods = html_methods.as_raw();
        mkr_cHtmlNode = h_node.as_raw();
        mkr_cHtmlDocument = h_document.as_raw();
        mkr_cHtmlElement = h_element.as_raw();
        mkr_cHtmlAttr = h_attr.as_raw();
        mkr_cHtmlText = h_text.as_raw();
        mkr_cHtmlComment = h_comment.as_raw();
        mkr_cHtmlCDATASection = h_cdata.as_raw();
        mkr_cHtmlProcessingInstruction = h_pi.as_raw();
        mkr_cHtmlDocumentType = h_doctype.as_raw();
        mkr_cHtmlDocumentFragment = h_fragment.as_raw();

        mkr_mXmlNodeMethods = xml_methods.as_raw();
        mkr_cXmlNode = x_node.as_raw();
        mkr_cXmlElement = x_element.as_raw();
        mkr_cXmlAttr = x_attr.as_raw();
        mkr_cXmlText = x_text.as_raw();
        mkr_cXmlComment = x_comment.as_raw();
        mkr_cXmlCDATASection = x_cdata.as_raw();
        mkr_cXmlProcessingInstruction = x_pi.as_raw();
        mkr_cXmlDocumentType = x_doctype.as_raw();
        mkr_cXmlDocumentFragment = x_fragment.as_raw();

        mkr_eError = err.as_raw();
        mkr_eXPathSyntaxError = xpath_syntax.as_raw();
        mkr_eXPathLimitExceeded = xpath_limit.as_raw();
        mkr_eCSSSyntaxError = css_syntax.as_raw();
        mkr_eXmlSyntaxError = xml_syntax.as_raw();
        mkr_eXmlLimitExceeded = xml_limit.as_raw();

        seal_leaves(
            mkr_mHtmlNodeMethods,
            &[
                mkr_cHtmlNode,
                mkr_cHtmlDocument,
                mkr_cHtmlElement,
                mkr_cHtmlAttr,
                mkr_cHtmlText,
                mkr_cHtmlComment,
                mkr_cHtmlCDATASection,
                mkr_cHtmlProcessingInstruction,
                mkr_cHtmlDocumentType,
                mkr_cHtmlDocumentFragment,
            ],
        );
        seal_leaves(
            mkr_mXmlNodeMethods,
            &[
                mkr_cXmlNode,
                mkr_cXmlElement,
                mkr_cXmlAttr,
                mkr_cXmlText,
                mkr_cXmlComment,
                mkr_cXmlCDATASection,
                mkr_cXmlProcessingInstruction,
                mkr_cXmlDocumentType,
                mkr_cXmlDocumentFragment,
            ],
        );

        /* The abstract bases are never constructed directly either: an instance
         * always wraps a live node, and `.new` would hand back one wrapping
         * nothing. XPathContext.new exists, but it is defined by
         * mkr_init_xpath and wraps a native context. */
        for base in [
            mkr_cNode,
            mkr_cDocument,
            element.as_raw(),
            attr.as_raw(),
            text.as_raw(),
            comment.as_raw(),
            cdata.as_raw(),
            pi.as_raw(),
            doctype.as_raw(),
            mkr_cDocumentFragment,
            mkr_cNodeSet,
            mkr_cXPathContext,
        ] {
            rb_sys::rb_undef_alloc_func(base);
        }

        /* The per-feature registrations, in the order the C called them: each
         * defines the methods of one subsystem onto the classes above. */
        crate::glue::html_node::mkr_init_node();
        crate::glue::doc::mkr_init_document();
        crate::glue::node_set::mkr_init_node_set();
        crate::glue::xpath::mkr_init_xpath();
        crate::glue::css::mkr_init_css();
        crate::glue::lexbor_css::mkr_init_lexbor_css();
        crate::glue::serialize::mkr_init_serialize();
        crate::glue::html_node::mkr_init_mutate();
        crate::glue::xml::mkr_init_xml();
        crate::glue::xml_node::mkr_init_xml_node();
    }

    makiri.define_singleton_method("__alloc_inject?", function!(alloc_inject_p, 0))?;
    makiri.define_singleton_method("__alloc_inject", function!(alloc_inject, 1))?;
    makiri.define_singleton_method("__alloc_inject_calls", function!(alloc_inject_calls, 0))?;
    m_xml.define_singleton_method("__decode", function!(xml_decode, 1))?;

    Ok(())
}
