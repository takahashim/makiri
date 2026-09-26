//! `Init_makiri` - the registration seam.
//!
//! Ruby loads the extension by calling `Init_makiri`, and this is it:
//! `#[magnus::init]` generates a symbol with exactly that name and the C ABI.
//! The contract "export only `Init_makiri`" (plus rb-sys's `ruby_abi_version`)
//! is enforced at the source: the macro's output is the crate's only
//! `#[no_mangle]` item, so rustc emits no other exported name. extconf's
//! post-link trim is belt-and-braces on macOS, not the mechanism, and
//! `rake symbols` checks the result.
//!
//! # The class VALUEs are written once
//!
//! Forty-seven classes and modules are created here and read from a dozen other
//! modules. In C they were plain globals; they are the same object with the same
//! contract here. Thirty-six are exported because something outside this file
//! reads them; the other eleven exist only long enough to build the hierarchy
//! and stay local.
//!
//! They are written exactly once, during `init`, before any Ruby code can run,
//! and only read afterwards - the argument the C's plain globals relied on. An
//! [`RbConst`] is typed by what it holds and roots it with the GC, so reading
//! one takes no `unsafe` and no type check.
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

use std::sync::OnceLock;

use magnus::rb_sys::AsRawValue;
use magnus::{
    function,
    value::{Opaque, ReprValue},
    Class, Error, ExceptionClass, Module, Object, RClass, RModule, Ruby, Value,
};

use crate::bridge::ruby::{gvl_ruby, VALUE};

/* ------------------------------------------------------------------ *
 * the classes and modules other modules read                         *
 * ------------------------------------------------------------------ *
 *
 * Written once by `init`, read by the glue modules that raise or check against
 * them. */

/// A class, module or exception class `init` defines, for the glue to read.
///
/// Typed: an `RbConst<RClass>` can only ever hold an `RClass`, so a read hands
/// the handle back with no type check to fail. It is written once, by `init`,
/// which also registers the object with the GC - so it stays alive and unmoved
/// even if Ruby code removes the constant that named it.
///
/// Unset is the state before `Init_makiri`, when none of the methods that read
/// one exist yet. A read in that state answers `None` (or the documented
/// fallback of the reader below) rather than panicking.
pub struct RbConst<T>(OnceLock<Opaque<T>>);

impl<T: ReprValue> RbConst<T> {
    const fn new() -> RbConst<T> {
        RbConst(OnceLock::new())
    }

    /// Record `v`, rooted for the rest of the process. `init` is the only
    /// writer, and a second write is refused rather than replacing the first.
    fn set(&self, v: T) -> Result<(), Error> {
        magnus::gc::register_mark_object(v);
        self.0.set(Opaque::from(v)).map_err(|_| {
            Error::new(
                gvl_ruby().exception_runtime_error(),
                "Makiri: a class handle was set twice",
            )
        })
    }

    /// The object, or `None` before `Init_makiri`.
    #[inline]
    pub fn get(&self) -> Option<T> {
        self.0.get().map(|o| gvl_ruby().get_inner(*o))
    }

    /// The object, for the registration code `init` runs once it is set.
    pub fn defined(&self) -> Result<T, Error> {
        self.get().ok_or_else(|| {
            Error::new(
                gvl_ruby().exception_runtime_error(),
                "Makiri: a class handle was read before Init_makiri",
            )
        })
    }

    /// The object as Ruby's handle, for a C call. `0` before `Init_makiri` -
    /// Ruby's `false`, which a TypedData wrap reads as "no class".
    #[inline]
    pub fn raw(&self) -> VALUE {
        self.get().map_or(0, |v| v.as_raw())
    }
}

impl RbConst<ExceptionClass> {
    /// The class to raise. Before `Init_makiri` - when nothing that raises one
    /// of these can run - it is `RuntimeError`, so a raise still raises.
    #[inline]
    pub fn exception(&self) -> ExceptionClass {
        self.get()
            .unwrap_or_else(|| gvl_ruby().exception_runtime_error())
    }
}

macro_rules! exported {
    ($ty:ty: $($name:ident),* $(,)?) => {
        $(
            pub static $name: RbConst<$ty> = RbConst::new();
        )*
    };
}

exported! { RClass:
    CLASS_NODE, CLASS_DOCUMENT, CLASS_DOCUMENT_FRAGMENT, CLASS_NODE_SET, CLASS_XPATH_CONTEXT,
    CLASS_HTML_NODE, CLASS_HTML_DOCUMENT, CLASS_HTML_ELEMENT,
    CLASS_HTML_ATTR, CLASS_HTML_TEXT, CLASS_HTML_COMMENT, CLASS_HTML_CDATA_SECTION,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_DOCUMENT_FRAGMENT,
    CLASS_XML_NODE, CLASS_XML_DOCUMENT, CLASS_XML_ELEMENT,
    CLASS_XML_ATTR, CLASS_XML_TEXT, CLASS_XML_COMMENT, CLASS_XML_CDATA_SECTION,
    CLASS_XML_PROCESSING_INSTRUCTION, CLASS_XML_DOCUMENT_TYPE, CLASS_XML_DOCUMENT_FRAGMENT,
}

exported! { RModule: MOD_XML, MOD_LEXBOR, MOD_HTML_NODE_METHODS, MOD_XML_NODE_METHODS }

exported! { ExceptionClass:
    EXC_ERROR, EXC_INTERNAL_ERROR, EXC_XPATH_SYNTAX_ERROR, EXC_XPATH_LIMIT_EXCEEDED,
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
        crate::falloc::alloc_inject_arm(nth);
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
        Ok(crate::falloc::alloc_inject_call_count())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        Err(Error::new(
            ruby.exception_not_imp_error(),
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__panic(kind)` - panic on purpose, so the suite can prove a panic
/// reaches Ruby as an exception instead of killing the process.
///
/// It exists because that property has no other test. `panic = "unwind"` plus
/// magnus's `catch_unwind` is what makes a panic a Ruby `fatal`; going back to
/// `abort`, or losing the unwind somewhere, would turn every example green and
/// the gem lethal. `spec/panic_spec.rb` calls this and expects to catch it.
///
/// Kinds 0..3 are the four ways the crate could actually panic: an explicit
/// `panic!`, an out-of-bounds index, an arithmetic overflow (release keeps
/// `overflow-checks` on), and an `unwrap` on `None`. Kind 4 panics BELOW a C
/// frame, where the unwind would abort if it were not latched. Kind 5 goes
/// through `bridge::ruby::entry`, so it raises `Makiri::InternalError` - what
/// the entry points exposed to untrusted input do.
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unnecessary_literal_unwrap,
    reason = "this function's whole purpose is to panic in each of these ways"
)]
fn panic_probe(ruby: &Ruby, kind: i64) -> Result<(), Error> {
    match kind {
        0 => panic!("Makiri.__panic(0): deliberate panic"),
        1 => {
            let v: Vec<u8> = vec![1, 2, 3];
            let _ = v[kind as usize * 100];
        }
        2 => {
            let x = kind as usize - 2;
            let _ = x - 1;
        }
        3 => {
            let n: Option<u8> = None;
            let _ = n.unwrap();
        }
        4 => {
            /* With the GVL released, so the panic starts below a C frame - the
             * largest such body in the crate is the parser itself.
             * `bridge::gvl` latches it and re-raises once the GVL is back. */
            crate::bridge::gvl::without_gvl(|| panic!("Makiri.__panic(4): panic below the GVL"))?;
        }
        5 => {
            /* Through `bridge::ruby::entry`, the wrapper the untrusted-input
             * entry points carry: the panic becomes `Makiri::InternalError`
             * instead of Ruby's unrescuable `fatal`. */
            return crate::bridge::ruby::entry(|| -> Result<(), Error> {
                panic!("Makiri.__panic(5): panic inside a guarded entry")
            });
        }
        _ => {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "__panic: kind must be 0..5",
            ))
        }
    }
    Ok(())
}

/// `Makiri::XML.__decode(str)` - the strict input decode in isolation, without
/// the tokenizer or the tree builder (`spec/xml_decode_spec.rb`).
fn xml_decode(_ruby: &Ruby, str: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
        let s = crate::bridge::ruby::string_of(str)?;
        /* decode-only: no arena, no budget */
        crate::bridge::xml_decode::xml_decode_input_value(s, None).map(|d| d.as_value())
    })
}

/* ------------------------------------------------------------------ *
 * Init_makiri                                                        *
 * ------------------------------------------------------------------ */

/// Define class `name` under `under` with superclass `super_`, and record it in
/// `slot` in the same call.
///
/// Definition and registration were separate, so a forgotten `slot.set` was
/// found only later as a "read before Init_makiri"; here the class cannot exist
/// without its handle.
fn define_into(
    under: RModule,
    name: &str,
    super_: RClass,
    slot: &RbConst<RClass>,
) -> Result<RClass, Error> {
    let class = under.define_class(name, super_)?;
    slot.set(class)?;
    Ok(class)
}

/// Give every leaf the representation's reader module and take away its
/// allocator.
///
/// The two go together: a leaf carries the readers because it wraps a live
/// node, and loses `.new` for the same reason.
fn seal_leaves(methods: RModule, leaves: &[RClass]) -> Result<(), Error> {
    for &leaf in leaves {
        leaf.include_module(methods)?;
        leaf.undef_default_alloc_func();
    }
    Ok(())
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
     * below; these exist so `is_a?(Makiri::Element)` holds across both. The
     * exported ones register as they are defined; the rest are local. */
    let node = define_into(makiri, "Node", ruby.class_object(), &CLASS_NODE)?;
    let document = define_into(makiri, "Document", node, &CLASS_DOCUMENT)?;
    let element = makiri.define_class("Element", node)?;
    let attr = makiri.define_class("Attr", node)?;
    let text = makiri.define_class("Text", node)?;
    let comment = makiri.define_class("Comment", node)?;
    let cdata = makiri.define_class("CDATASection", node)?;
    let pi = makiri.define_class("ProcessingInstruction", node)?;
    let doctype = makiri.define_class("DocumentType", node)?;
    let fragment = define_into(makiri, "DocumentFragment", node, &CLASS_DOCUMENT_FRAGMENT)?;
    let node_set = define_into(makiri, "NodeSet", ruby.class_object(), &CLASS_NODE_SET)?;
    let xpath_context = define_into(
        makiri,
        "XPathContext",
        ruby.class_object(),
        &CLASS_XPATH_CONTEXT,
    )?;

    let m_xpath = makiri.define_module("XPath")?;
    let m_css = makiri.define_module("CSS")?;
    let m_xml = makiri.define_module("XML")?;
    let m_lexbor = makiri.define_module("Lexbor")?;
    MOD_XML.set(m_xml)?;
    MOD_LEXBOR.set(m_lexbor)?;

    /* Makiri::HTML - the Lexbor-backed leaves. */
    let m_html = makiri.define_module("HTML")?;
    let html_methods = m_html.define_module("NodeMethods")?;
    MOD_HTML_NODE_METHODS.set(html_methods)?;
    let html_leaves = [
        define_into(m_html, "Node", node, &CLASS_HTML_NODE)?,
        define_into(m_html, "Document", document, &CLASS_HTML_DOCUMENT)?,
        define_into(m_html, "Element", element, &CLASS_HTML_ELEMENT)?,
        define_into(m_html, "Attr", attr, &CLASS_HTML_ATTR)?,
        define_into(m_html, "Text", text, &CLASS_HTML_TEXT)?,
        define_into(m_html, "Comment", comment, &CLASS_HTML_COMMENT)?,
        define_into(m_html, "CDATASection", cdata, &CLASS_HTML_CDATA_SECTION)?,
        define_into(
            m_html,
            "ProcessingInstruction",
            pi,
            &CLASS_HTML_PROCESSING_INSTRUCTION,
        )?,
        define_into(m_html, "DocumentType", doctype, &CLASS_HTML_DOCUMENT_TYPE)?,
        define_into(
            m_html,
            "DocumentFragment",
            fragment,
            &CLASS_HTML_DOCUMENT_FRAGMENT,
        )?,
    ];

    /* Makiri::XML - the arena-backed leaves. XML::Document is one of them: it
     * carries no HTML readers, so `is_a?(Makiri::Document)` holds while the
     * structural surface comes from the module `seal_leaves` includes below.
     * DocumentType descends from the SHARED base, not XML::Node, so
     * `is_a?(Makiri::DocumentType)` holds for both representations; it is
     * still an XML leaf. */
    let xml_methods = m_xml.define_module("NodeMethods")?;
    MOD_XML_NODE_METHODS.set(xml_methods)?;
    let xml_leaves = [
        define_into(m_xml, "Node", node, &CLASS_XML_NODE)?,
        define_into(m_xml, "Document", document, &CLASS_XML_DOCUMENT)?,
        define_into(m_xml, "Element", element, &CLASS_XML_ELEMENT)?,
        define_into(m_xml, "Attr", attr, &CLASS_XML_ATTR)?,
        define_into(m_xml, "Text", text, &CLASS_XML_TEXT)?,
        define_into(m_xml, "Comment", comment, &CLASS_XML_COMMENT)?,
        define_into(m_xml, "CDATASection", cdata, &CLASS_XML_CDATA_SECTION)?,
        define_into(
            m_xml,
            "ProcessingInstruction",
            pi,
            &CLASS_XML_PROCESSING_INSTRUCTION,
        )?,
        define_into(m_xml, "DocumentType", doctype, &CLASS_XML_DOCUMENT_TYPE)?,
        define_into(
            m_xml,
            "DocumentFragment",
            fragment,
            &CLASS_XML_DOCUMENT_FRAGMENT,
        )?,
    ];

    /* `define_error`, not `define_class`: an exception class is an
     * `ExceptionClass` in magnus, and raising through one is the only thing
     * these are for. LimitExceeded descends from XPath::SyntaxError (so
     * rescuing the syntax error catches the budget too), while its XML
     * counterpart descends from Error - that asymmetry is deliberate and
     * predates the port. */
    let err = makiri.define_error("Error", ruby.exception_standard_error())?;

    /* `InternalError` descends from Exception, NOT from StandardError, and that
     * is the whole point. It carries a Rust panic - a broken invariant, not a
     * bad argument - so a bare `rescue => e` must not swallow it the way it
     * would a bad selector, while a host that wants to turn one request into a
     * 500 can still `rescue Makiri::InternalError`. That is what Ruby's own
     * `fatal` gives, minus the part where it cannot be rescued at all.
     * `bridge::ruby::entry` is what raises it; see `crate::caught`. */
    let internal = makiri.define_error("InternalError", ruby.exception_exception())?;
    let xpath_syntax = m_xpath.define_error("SyntaxError", err)?;
    let xpath_limit = m_xpath.define_error("LimitExceeded", xpath_syntax)?;
    let css_syntax = m_css.define_error("SyntaxError", err)?;
    let xml_syntax = m_xml.define_error("SyntaxError", err)?;
    let xml_limit = m_xml.define_error("LimitExceeded", err)?;

    /* Recorded before anything that reads them is registered: the glue's
     * `init`s below read these, and so does every method they define. The
     * classes and node-behaviour modules are recorded where they are defined. */
    EXC_ERROR.set(err)?;
    EXC_INTERNAL_ERROR.set(internal)?;
    EXC_XPATH_SYNTAX_ERROR.set(xpath_syntax)?;
    EXC_XPATH_LIMIT_EXCEEDED.set(xpath_limit)?;
    EXC_CSS_SYNTAX_ERROR.set(css_syntax)?;
    EXC_XML_SYNTAX_ERROR.set(xml_syntax)?;
    EXC_XML_LIMIT_EXCEEDED.set(xml_limit)?;

    seal_leaves(html_methods, &html_leaves)?;
    seal_leaves(xml_methods, &xml_leaves)?;

    /* The abstract bases are never constructed directly either: an instance
     * always wraps a live node, and `.new` would hand back one wrapping
     * nothing. XPathContext.new exists, but it is defined by
     * init_xpath and wraps a native context. */
    for base in [
        node,
        document,
        element,
        attr,
        text,
        comment,
        cdata,
        pi,
        doctype,
        fragment,
        node_set,
        xpath_context,
    ] {
        base.undef_default_alloc_func();
    }

    /* One registration per feature, each defining its methods onto the
     * classes above. The order is free: every class and module they touch
     * exists by now, and no two define the same name. */
    crate::glue::html_node::init()?;
    crate::glue::html_doc::init_html_doc()?;
    crate::glue::xml_node::init()?;
    crate::glue::xml_doc::init_xml_doc()?;
    crate::glue::node_set::init_node_set()?;
    crate::glue::xpath_context::init_xpath_context()?;
    crate::glue::query::init_xpath()?;
    crate::glue::stylesheet::init_lexbor_css(ruby)?;

    makiri.define_singleton_method("__alloc_inject?", function!(alloc_inject_p, 0))?;
    makiri.define_singleton_method("__panic", function!(panic_probe, 1))?;
    makiri.define_singleton_method("__alloc_inject", function!(alloc_inject, 1))?;
    makiri.define_singleton_method("__alloc_inject_calls", function!(alloc_inject_calls, 0))?;
    m_xml.define_singleton_method("__decode", function!(xml_decode, 1))?;

    Ok(())
}
