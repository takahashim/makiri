//! `Init_makiri` - the registration seam (makiri.c).
//!
//! Ruby loads the extension by calling `Init_makiri`, and this is it:
//! `#[magnus::init]` generates a symbol with exactly that name and the C ABI, so
//! the contract "export only `Init_makiri`" is unchanged - it is enforced by the
//! linker flag, as it always was, not by the language the function is written in.
//!
//! # The class VALUEs are written once
//!
//! Forty-six classes and modules are created here and read from a dozen other
//! modules. In C they were plain globals; they are the same object with the same
//! contract here. Thirty-five are exported because something outside this file
//! reads them; the other eleven exist only long enough to build the hierarchy
//! and stay local.
//!
//! They are written exactly once, during `init`, before any Ruby code can run,
//! and only read afterwards - the argument the C's plain globals relied on. An
//! [`RbConst`] states it once, so reading one takes no `unsafe`.
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

#![allow(unsafe_code)]

use core::sync::atomic::{AtomicUsize, Ordering};
use magnus::rb_sys::AsRawValue;

use magnus::{function, Class, Error, ExceptionClass, Module, Object, RClass, RModule, Ruby, Value};
use crate::bridge::ruby::VALUE;

/* ------------------------------------------------------------------ *
 * the classes and modules other modules read                         *
 * ------------------------------------------------------------------ *
 *
 * Written once by `init`, read by the glue modules that raise or check against
 * them. */

/// A class, module or exception class `init` defines, for the glue to read.
///
/// Written once, during `init`, before any Ruby code can run, and only read
/// afterwards. Every object one holds is a constant under `Makiri`, so it lives
/// for the rest of the process. Read before `init` it is `0` - Ruby's `false`,
/// a valid immediate rather than a dangling object.
pub struct RbConst(AtomicUsize);

const _: () = assert!(core::mem::size_of::<VALUE>() <= core::mem::size_of::<usize>());

impl RbConst {
    const fn new() -> RbConst {
        RbConst(AtomicUsize::new(0))
    }

    /// # Safety
    /// `v` must be a class or module that lives for the rest of the process.
    pub(crate) unsafe fn set(&self, v: VALUE) {
        self.0.store(v as usize, Ordering::Relaxed);
    }

    /// The object as Ruby's handle, for a C call.
    #[inline]
    pub fn raw(&self) -> VALUE {
        self.0.load(Ordering::Relaxed) as VALUE
    }

    /// The object as a value.
    #[inline]
    pub fn value(&self) -> Value {
        // SAFETY: `0` or, once `init` has run, a class that lives for the
        // process - see the type.
        unsafe { crate::bridge::ruby::value(self.raw()) }
    }

    pub fn class(&self) -> RClass {
        RClass::from_value(self.value()).expect("a Makiri class, after Init_makiri")
    }

    pub fn module(&self) -> RModule {
        RModule::from_value(self.value()).expect("a Makiri module, after Init_makiri")
    }

    pub fn exception(&self) -> ExceptionClass {
        ExceptionClass::from_value(self.value()).expect("a Makiri exception, after Init_makiri")
    }
}

/// Publish the `Makiri::XML::Document` class as the global the rest of the
/// extension reads.
///
/// Safe wrapper over [`RbConst::set`]: it runs once, from `Init_makiri`, with a
/// class that lives for the rest of the process.
pub(crate) fn record_xml_document_class(klass: magnus::RClass) {
    // SAFETY: a class that lives for the process, set once at init.
    unsafe { CLASS_XML_DOCUMENT.set(klass.as_raw()) };
}

macro_rules! exported {
    ($($name:ident),* $(,)?) => {
        $(
            pub static $name: RbConst = RbConst::new();
        )*
    };
}

exported! {
    CLASS_NODE, CLASS_DOCUMENT, CLASS_DOCUMENT_FRAGMENT, CLASS_NODE_SET, CLASS_XPATH_CONTEXT,
    MOD_XML, MOD_LEXBOR,
    MOD_HTML_NODE_METHODS, CLASS_HTML_NODE, CLASS_HTML_DOCUMENT, CLASS_HTML_ELEMENT,
    CLASS_HTML_ATTR, CLASS_HTML_TEXT, CLASS_HTML_COMMENT, CLASS_HTML_CDATA_SECTION,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_DOCUMENT_FRAGMENT,
    MOD_XML_NODE_METHODS, CLASS_XML_NODE, CLASS_XML_DOCUMENT, CLASS_XML_ELEMENT,
    CLASS_XML_ATTR, CLASS_XML_TEXT, CLASS_XML_COMMENT, CLASS_XML_CDATA_SECTION,
    CLASS_XML_PROCESSING_INSTRUCTION, CLASS_XML_DOCUMENT_TYPE, CLASS_XML_DOCUMENT_FRAGMENT,
    EXC_ERROR, EXC_XPATH_SYNTAX_ERROR, EXC_XPATH_LIMIT_EXCEEDED,
    EXC_CSS_SYNTAX_ERROR, EXC_XML_SYNTAX_ERROR, EXC_XML_LIMIT_EXCEEDED,
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
fn xml_decode(ruby: &Ruby, str: Value) -> Result<Value, Error> {
    let _ = ruby;
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let s = crate::bridge::ruby::string_of(str)?;
    /* decode-only: no arena, no budget */
    Ok(unsafe { crate::bridge::ruby::value(crate::bridge::xml_decode::xml_decode_input(s.as_raw(), 0)?) })
}

/* ------------------------------------------------------------------ *
 * Init_makiri                                                        *
 * ------------------------------------------------------------------ */

/// Give every leaf the representation's reader module and take away its
/// allocator.
///
/// The two go together: a leaf carries the readers because it wraps a live
/// node, and loses `.new` for the same reason.
fn seal_leaves(methods: RModule, leaves: &[RClass]) {
    for &leaf in leaves {
        leaf.include_module(methods)
            .expect("including the reader module");
        leaf.undef_default_alloc_func();
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
     * init_xml, because it backs a parse handle rather than a node. */
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
        CLASS_NODE.set(node.as_raw());
        CLASS_DOCUMENT.set(document.as_raw());
        CLASS_DOCUMENT_FRAGMENT.set(fragment.as_raw());
        CLASS_NODE_SET.set(node_set.as_raw());
        CLASS_XPATH_CONTEXT.set(xpath_context.as_raw());
        MOD_XML.set(m_xml.as_raw());
        MOD_LEXBOR.set(m_lexbor.as_raw());

        MOD_HTML_NODE_METHODS.set(html_methods.as_raw());
        CLASS_HTML_NODE.set(h_node.as_raw());
        CLASS_HTML_DOCUMENT.set(h_document.as_raw());
        CLASS_HTML_ELEMENT.set(h_element.as_raw());
        CLASS_HTML_ATTR.set(h_attr.as_raw());
        CLASS_HTML_TEXT.set(h_text.as_raw());
        CLASS_HTML_COMMENT.set(h_comment.as_raw());
        CLASS_HTML_CDATA_SECTION.set(h_cdata.as_raw());
        CLASS_HTML_PROCESSING_INSTRUCTION.set(h_pi.as_raw());
        CLASS_HTML_DOCUMENT_TYPE.set(h_doctype.as_raw());
        CLASS_HTML_DOCUMENT_FRAGMENT.set(h_fragment.as_raw());

        MOD_XML_NODE_METHODS.set(xml_methods.as_raw());
        CLASS_XML_NODE.set(x_node.as_raw());
        CLASS_XML_ELEMENT.set(x_element.as_raw());
        CLASS_XML_ATTR.set(x_attr.as_raw());
        CLASS_XML_TEXT.set(x_text.as_raw());
        CLASS_XML_COMMENT.set(x_comment.as_raw());
        CLASS_XML_CDATA_SECTION.set(x_cdata.as_raw());
        CLASS_XML_PROCESSING_INSTRUCTION.set(x_pi.as_raw());
        CLASS_XML_DOCUMENT_TYPE.set(x_doctype.as_raw());
        CLASS_XML_DOCUMENT_FRAGMENT.set(x_fragment.as_raw());

        EXC_ERROR.set(err.as_raw());
        EXC_XPATH_SYNTAX_ERROR.set(xpath_syntax.as_raw());
        EXC_XPATH_LIMIT_EXCEEDED.set(xpath_limit.as_raw());
        EXC_CSS_SYNTAX_ERROR.set(css_syntax.as_raw());
        EXC_XML_SYNTAX_ERROR.set(xml_syntax.as_raw());
        EXC_XML_LIMIT_EXCEEDED.set(xml_limit.as_raw());

        seal_leaves(
            MOD_HTML_NODE_METHODS.module(),
            &[
                CLASS_HTML_NODE.class(),
                CLASS_HTML_DOCUMENT.class(),
                CLASS_HTML_ELEMENT.class(),
                CLASS_HTML_ATTR.class(),
                CLASS_HTML_TEXT.class(),
                CLASS_HTML_COMMENT.class(),
                CLASS_HTML_CDATA_SECTION.class(),
                CLASS_HTML_PROCESSING_INSTRUCTION.class(),
                CLASS_HTML_DOCUMENT_TYPE.class(),
                CLASS_HTML_DOCUMENT_FRAGMENT.class(),
            ],
        );
        seal_leaves(
            MOD_XML_NODE_METHODS.module(),
            &[
                CLASS_XML_NODE.class(),
                CLASS_XML_ELEMENT.class(),
                CLASS_XML_ATTR.class(),
                CLASS_XML_TEXT.class(),
                CLASS_XML_COMMENT.class(),
                CLASS_XML_CDATA_SECTION.class(),
                CLASS_XML_PROCESSING_INSTRUCTION.class(),
                CLASS_XML_DOCUMENT_TYPE.class(),
                CLASS_XML_DOCUMENT_FRAGMENT.class(),
            ],
        );

        /* The abstract bases are never constructed directly either: an instance
         * always wraps a live node, and `.new` would hand back one wrapping
         * nothing. XPathContext.new exists, but it is defined by
         * init_xpath and wraps a native context. */
        for base in [
            CLASS_NODE.class(),
            CLASS_DOCUMENT.class(),
            element,
            attr,
            text,
            comment,
            cdata,
            pi,
            doctype,
            CLASS_DOCUMENT_FRAGMENT.class(),
            CLASS_NODE_SET.class(),
            CLASS_XPATH_CONTEXT.class(),
        ] {
            base.undef_default_alloc_func();
        }

        /* The per-feature registrations, in the order the C called them: each
         * defines the methods of one subsystem onto the classes above. */
        crate::glue::html_node::init_node();
        crate::glue::doc::init_document();
        crate::glue::node_set::init_node_set();
        crate::glue::xpath::init_xpath();
        crate::bridge::selectors::init_css();
        crate::lexbor::stylesheet::init_lexbor_css();
        crate::bridge::serialize::init_serialize();
        crate::glue::html_node::init_mutate();
        crate::glue::xml::init_xml();
        crate::glue::xml_node::init_xml_node();
    }

    makiri.define_singleton_method("__alloc_inject?", function!(alloc_inject_p, 0))?;
    makiri.define_singleton_method("__alloc_inject", function!(alloc_inject, 1))?;
    makiri.define_singleton_method("__alloc_inject_calls", function!(alloc_inject_calls, 0))?;
    m_xml.define_singleton_method("__decode", function!(xml_decode, 1))?;

    Ok(())
}
