//! `Init_makiri` - the registration seam.
//!
//! Ruby loads the extension by calling `Init_makiri`, and this is it: an
//! `extern "C"` function of exactly that name. The contract "export only
//! `Init_makiri`" (plus rb-sys's `ruby_abi_version`) is enforced at the
//! source: it is the crate's only `#[no_mangle]` item, so rustc emits no
//! other exported name. extconf's
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
//! Every class here also loses its allocator (`define_data_class`). These
//! objects are created only from Rust, wrapping a live node; `.new` would hand
//! back one wrapping nothing.

use crate::bridge::rb::{DataClass, Error, ExceptionClass, Module, Object, RModule, Ruby, Value};
use crate::function;

/* ------------------------------------------------------------------ *
 * the classes and modules other modules read                         *
 * ------------------------------------------------------------------ *
 *
 * Written once by `init`, read by the glue modules that raise or check against
 * them. */

macro_rules! exported {
    ($ty:ty: $($name:ident),* $(,)?) => {
        $(
            pub static $name: RbConst<$ty> = RbConst::new();
        )*
    };
}

exported! { DataClass:
    CLASS_NODE, CLASS_DOCUMENT, CLASS_DOCUMENT_FRAGMENT, CLASS_NODE_SET, CLASS_XPATH_CONTEXT,
    CLASS_HTML_NODE, CLASS_HTML_DOCUMENT, CLASS_HTML_ELEMENT,
    CLASS_HTML_ATTR, CLASS_HTML_TEXT, CLASS_HTML_COMMENT, CLASS_HTML_CDATA_SECTION,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_DOCUMENT_FRAGMENT,
    CLASS_XML_NODE, CLASS_XML_DOCUMENT, CLASS_XML_ELEMENT,
    CLASS_XML_ATTR, CLASS_XML_TEXT, CLASS_XML_COMMENT, CLASS_XML_CDATA_SECTION,
    CLASS_XML_PROCESSING_INSTRUCTION, CLASS_XML_DOCUMENT_TYPE, CLASS_XML_DOCUMENT_FRAGMENT,
}

exported! { RModule: MOD_XML, MOD_LEXBOR, MOD_HTML_NODE_METHODS, MOD_XML_NODE_METHODS }

/// `Makiri::InternalError`: the class `nupur` raises for a broken invariant,
/// registered with it here so the layer names no class of Makiri's.
pub use crate::nupur::rb::error::INTERNAL_ERROR as EXC_INTERNAL_ERROR;

/// The rooted handle each of these is (`nupur::rb::RbConst`).
pub use crate::nupur::rb::RbConst;

exported! { ExceptionClass:
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
fn alloc_inject(nth: Value) -> Result<(), Error> {
    let nth = crate::bridge::ruby::integer_arg(nth)?.to_i64()?;
    #[cfg(feature = "alloc-inject")]
    {
        crate::falloc::alloc_inject_arm(nth);
        Ok(())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        let _ = nth;
        Err(Error::new(
            crate::bridge::rb::Builtin::NotImplementedError,
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__alloc_inject_calls` - how many core allocations a workload
/// attempted, armed or not, so the sweep can size itself from a disarmed run.
fn alloc_inject_calls() -> Result<u64, Error> {
    #[cfg(feature = "alloc-inject")]
    {
        Ok(crate::falloc::alloc_inject_call_count())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        Err(Error::new(
            crate::bridge::rb::Builtin::NotImplementedError,
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__raise_inject(n)` - arm "the nth raising C call raises
/// `NoMemoryError` in place of its allocation" (`bridge::raw`'s allocating
/// wrappers; `script/check_raise_injection.rb`).
fn raise_inject(nth: Value) -> Result<(), Error> {
    let nth = crate::bridge::ruby::integer_arg(nth)?.to_i64()?;
    #[cfg(feature = "alloc-inject")]
    {
        crate::falloc::raise_inject_arm(nth);
        Ok(())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        let _ = nth;
        Err(Error::new(
            crate::bridge::rb::Builtin::NotImplementedError,
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__raise_inject_calls` - how many raising C calls a workload made,
/// armed or not, so the sweep can size itself from a disarmed run.
fn raise_inject_calls() -> Result<u64, Error> {
    #[cfg(feature = "alloc-inject")]
    {
        Ok(crate::falloc::raise_inject_call_count())
    }
    #[cfg(not(feature = "alloc-inject"))]
    {
        Err(Error::new(
            crate::bridge::rb::Builtin::NotImplementedError,
            "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)",
        ))
    }
}

/// `Makiri.__debug_assertions?` - whether this extension was built with
/// `debug_assertions` (a dev-profile build, `MAKIRI_CARGO_PROFILE=dev`), so
/// `rake debug` can refuse a run whose `debug_assert!`s are compiled out.
fn debug_assertions_p() -> bool {
    cfg!(debug_assertions)
}

/// `Makiri.__borrowed?(doc)` - whether `doc`'s content is borrowed, which
/// between Ruby calls it never is: what the raise sweep checks after each
/// injected raise. Any build.
fn borrowed_p(doc: Value) -> Result<bool, Error> {
    crate::bridge::wrapper::document_borrowed(doc)
}

/// `Makiri.__panic(kind)` - panic on purpose, so the suite can prove a panic
/// reaches Ruby as an exception instead of killing the process.
///
/// It exists because that property has no other test. `panic = "unwind"` plus
/// the trampoline's `catch_unwind` (`bridge::rb::method::finish`) is what makes
/// a panic a Ruby exception; going back to `abort`, or losing the unwind
/// somewhere, would turn every example green and the gem lethal. `spec/panic_spec.rb` calls this and expects to catch it.
///
/// Kinds 0..3 are the four ways the crate could actually panic: an explicit
/// `panic!`, an out-of-bounds index, an arithmetic overflow (release keeps
/// `overflow-checks` on), and an `unwrap` on `None`. Kind 4 panics BELOW a C
/// frame, where the unwind would abort if it were not latched. Kind 5 panics
/// in the method body itself. Kind 6 panics in the token-done path of a real
/// document parse (`html::tree::parse_with_panicking_hook`). Every kind raises
/// `Makiri::InternalError`, from the trampoline (`bridge::rb::method::finish`).
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unnecessary_literal_unwrap,
    reason = "this function's whole purpose is to panic in each of these ways"
)]
fn panic_probe(ruby: &Ruby, kind: Value) -> Result<(), Error> {
    let kind = crate::bridge::ruby::integer_arg(kind)?.to_i64()?;
    /* 1 and 2, where it is read as a `usize` below. */
    let as_index = usize::try_from(kind).unwrap_or(0);
    match kind {
        0 => panic!("Makiri.__panic(0): deliberate panic"),
        1 => {
            /* The test hook's own allocation: a panic is what it is for. */
            #[cfg_attr(dylint_lib = "makiri_lints", allow(infallible_allocation))]
            let v: Vec<u8> = vec![1, 2, 3];
            let _ = v[as_index * 100];
        }
        2 => {
            let x = as_index - 2;
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
            crate::bridge::gvl::without_gvl(ruby, || {
                panic!("Makiri.__panic(4): panic below the GVL")
            })?;
        }
        5 => {
            /* In a method's body, as any method's: its trampoline
             * (`bridge::rb::method::finish`) makes the panic
             * `Makiri::InternalError` instead of Ruby's unrescuable `fatal`. */
            panic!("Makiri.__panic(5): panic inside a method body")
        }
        6 => {
            /* Inside the tokenizer, in the token-done hook every HTML parse
             * runs: the hook latches it and stops the parse, the parse
             * re-raises it, and the trampoline makes it
             * `Makiri::InternalError`. */
            crate::html_dom::post_parse::parse_with_panicking_hook();
        }
        7 => {
            /* Part-way through writing a String in place
             * (`nupur::string::utf8_str_filled`): the panic unwinds through its frame,
             * whose guard must empty the 12345-byte String it leaves for the
             * GC, rather than leave its unwritten bytes for `ObjectSpace`. */
            crate::nupur::string::utf8_str_filled(ruby, 12345, |out| {
                let _ = out.push(b"written");
                panic!("Makiri.__panic(7): panic while a String is filled")
            })?;
        }
        _ => {
            return Err(Error::new(
                crate::bridge::rb::Builtin::ArgumentError,
                "__panic: kind must be 0..7",
            ))
        }
    }
    Ok(())
}

/// `Makiri.__drop_exit { ... }` - yield once and drop what the block's exit
/// or exception came back as: a method that loses an `Err`, which the lints
/// keep out of the extension, and so made by nupur
/// (`raw::yield_dropping_error_for_test`). A dropped `throw`, `break` or
/// thread kill must still be resumed - by the trampoline, as the method
/// returns (`spec/pending_exit_spec.rb`) - never lost, nor left pending for
/// another method or thread. Any build.
fn drop_exit(ruby: &Ruby) {
    crate::bridge::raw::yield_dropping_error_for_test(ruby);
}

/// `Makiri::XML.__decode(str)` - the strict input decode in isolation, without
/// the tokenizer or the tree builder (`spec/xml_decode_spec.rb`).
fn xml_decode(ruby: &Ruby, str: Value) -> Result<Value, Error> {
    let s = crate::bridge::ruby::string_arg(str)?;
    /* decode-only: no arena, no budget */
    crate::bridge::xml_decode::xml_decode_input(ruby, s, None)
        .and_then(|d| crate::bridge::string::utf8_str(ruby, d.as_slice()))
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
    super_: Option<DataClass>,
    slot: &RbConst<DataClass>,
) -> Result<DataClass, Error> {
    let class = under.define_data_class(name, super_)?;
    slot.set(class)?;
    Ok(class)
}

/// Give every leaf the representation's reader module: a leaf carries the
/// readers because it wraps a live node.
fn seal_leaves(methods: RModule, leaves: &[DataClass]) -> Result<(), Error> {
    for &leaf in leaves {
        leaf.include_module(methods)?;
    }
    Ok(())
}

/// What Ruby calls when it loads `makiri.bundle`: `Init_<basename>`, so the
/// name is load-bearing - and the crate's one `#[no_mangle]` item, which is
/// what keeps every other name out of the export table (`rake symbols`).
///
/// An error from [`init`] is raised, as a `LoadError` or whatever the failing
/// definition raised, once `init` has returned. A panic is Ruby's `fatal`
/// rather than `Makiri::InternalError`: that class may not exist yet, and an
/// extension that stopped half-way through defining itself must not be used.
#[allow(non_snake_case, unsafe_code)]
#[no_mangle]
pub extern "C" fn Init_makiri() {
    // SAFETY: this is the `Init_` function Ruby calls, on its thread with the
    // GVL held.
    unsafe { crate::bridge::rb::method::run_init(check, init) };
}

/// Before anything touches the interpreter's structs: a binary built for
/// another Ruby misreads them (`nupur::abi::check_running_api`). Its refusal is a
/// `LoadError`, with nothing defined yet; anything that stops `init` after
/// it is `fatal` (`run_init`).
fn check(ruby: &Ruby) -> Result<(), Error> {
    crate::nupur::abi::check_running_api(
        "makiri",
        "`bundle exec rake clean compile` in a checkout, or reinstall the gem",
    )?;
    crate::nupur::abi::check_raise_state(ruby)
}

fn init(ruby: &Ruby) -> Result<(), Error> {
    #[cfg(makiri_no_unwind)]
    crate::no_unwind::self_test();
    #[cfg(nupur_no_unwind)]
    crate::nupur::no_unwind::self_test();
    /* The OOM sweep's answers, for `nupur::raw`'s allocating calls and a
     * checked copy's heap allocation. */
    #[cfg(feature = "alloc-inject")]
    crate::nupur::inject::install(crate::nupur::inject::Injectors {
        raise: crate::falloc::raise_inject_should_raise,
        alloc: crate::falloc::allocation_should_fail,
    });
    let makiri = ruby.define_module("Makiri")?;
    crate::bridge::wrapper::init_ivars(ruby)?;

    // What this binary was built as, for `lib/makiri.rb` to check against the
    // gem it was loaded by and the Ruby it was loaded into - a stale
    // `lib/makiri/makiri.bundle` in a checkout is read by whichever Ruby, and
    // whichever version of the Ruby code, requires it.
    let native_version =
        crate::bridge::string::utf8_str(ruby, env!("MAKIRI_GEM_VERSION").as_bytes())?;
    crate::bridge::rb::freeze(native_version)?;
    makiri.const_set(ruby, "NATIVE_VERSION", native_version)?;
    let native_ruby = crate::bridge::string::utf8_str(
        ruby,
        crate::nupur::abi::ruby_api_version().as_str().as_bytes(),
    )?;
    crate::bridge::rb::freeze(native_ruby)?;
    makiri.const_set(ruby, "NATIVE_RUBY_API_VERSION", native_ruby)?;

    /* The abstract bases. Concrete nodes are the HTML::* / XML::* leaves
     * below; these exist so `is_a?(Makiri::Element)` holds across both. The
     * exported ones register as they are defined; the rest are local. */
    let node = define_into(makiri, "Node", None, &CLASS_NODE)?;
    let document = define_into(makiri, "Document", Some(node), &CLASS_DOCUMENT)?;
    let element = makiri.define_data_class("Element", Some(node))?;
    let attr = makiri.define_data_class("Attr", Some(node))?;
    let text = makiri.define_data_class("Text", Some(node))?;
    let comment = makiri.define_data_class("Comment", Some(node))?;
    let cdata = makiri.define_data_class("CDATASection", Some(node))?;
    let pi = makiri.define_data_class("ProcessingInstruction", Some(node))?;
    let doctype = makiri.define_data_class("DocumentType", Some(node))?;
    let fragment = define_into(
        makiri,
        "DocumentFragment",
        Some(node),
        &CLASS_DOCUMENT_FRAGMENT,
    )?;
    define_into(makiri, "NodeSet", None, &CLASS_NODE_SET)?;
    define_into(makiri, "XPathContext", None, &CLASS_XPATH_CONTEXT)?;

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
        define_into(m_html, "Node", Some(node), &CLASS_HTML_NODE)?,
        define_into(m_html, "Document", Some(document), &CLASS_HTML_DOCUMENT)?,
        define_into(m_html, "Element", Some(element), &CLASS_HTML_ELEMENT)?,
        define_into(m_html, "Attr", Some(attr), &CLASS_HTML_ATTR)?,
        define_into(m_html, "Text", Some(text), &CLASS_HTML_TEXT)?,
        define_into(m_html, "Comment", Some(comment), &CLASS_HTML_COMMENT)?,
        define_into(
            m_html,
            "CDATASection",
            Some(cdata),
            &CLASS_HTML_CDATA_SECTION,
        )?,
        define_into(
            m_html,
            "ProcessingInstruction",
            Some(pi),
            &CLASS_HTML_PROCESSING_INSTRUCTION,
        )?,
        define_into(
            m_html,
            "DocumentType",
            Some(doctype),
            &CLASS_HTML_DOCUMENT_TYPE,
        )?,
        define_into(
            m_html,
            "DocumentFragment",
            Some(fragment),
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
        define_into(m_xml, "Node", Some(node), &CLASS_XML_NODE)?,
        define_into(m_xml, "Document", Some(document), &CLASS_XML_DOCUMENT)?,
        define_into(m_xml, "Element", Some(element), &CLASS_XML_ELEMENT)?,
        define_into(m_xml, "Attr", Some(attr), &CLASS_XML_ATTR)?,
        define_into(m_xml, "Text", Some(text), &CLASS_XML_TEXT)?,
        define_into(m_xml, "Comment", Some(comment), &CLASS_XML_COMMENT)?,
        define_into(m_xml, "CDATASection", Some(cdata), &CLASS_XML_CDATA_SECTION)?,
        define_into(
            m_xml,
            "ProcessingInstruction",
            Some(pi),
            &CLASS_XML_PROCESSING_INSTRUCTION,
        )?,
        define_into(
            m_xml,
            "DocumentType",
            Some(doctype),
            &CLASS_XML_DOCUMENT_TYPE,
        )?,
        define_into(
            m_xml,
            "DocumentFragment",
            Some(fragment),
            &CLASS_XML_DOCUMENT_FRAGMENT,
        )?,
    ];

    /* `define_error`, not `define_class`: an exception class is an
     * `ExceptionClass`, and raising through one is the only thing these are
     * for. LimitExceeded descends from XPath::SyntaxError (so
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
     * Every method's trampoline (`bridge::rb::method::finish`) raises it;
     * see `crate::caught`. */
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

    /* One registration per feature, each defining its methods onto the
     * classes above. The order is free: every class and module they touch
     * exists by now, and no two define the same name. */
    crate::glue::html_node::init(ruby)?;
    crate::glue::html_doc::init_html_doc(ruby)?;
    crate::glue::xml_node::init(ruby)?;
    crate::glue::xml_doc::init_xml_doc(ruby)?;
    crate::glue::node_set::init_node_set(ruby)?;
    crate::glue::xpath_context::init_xpath_context(ruby)?;
    crate::glue::query::init_xpath(ruby)?;
    crate::glue::stylesheet::init_lexbor_css(ruby)?;

    /* The recursion checks in css/ and xpath/ consult this without knowing
     * Ruby exists; see crate::stack for why a depth count alone does not
     * bound the native stack. */
    crate::stack::install(crate::bridge::stack::probe(ruby));

    makiri.define_singleton_method("__alloc_inject?", function!(alloc_inject_p, 0))?;
    makiri.define_singleton_method("__panic", function!(panic_probe, 1))?;
    makiri.define_singleton_method("__drop_exit", function!(drop_exit, 0))?;
    makiri.define_singleton_method("__alloc_inject", function!(alloc_inject, 1))?;
    makiri.define_singleton_method("__alloc_inject_calls", function!(alloc_inject_calls, 0))?;
    makiri.define_singleton_method("__raise_inject", function!(raise_inject, 1))?;
    makiri.define_singleton_method("__raise_inject_calls", function!(raise_inject_calls, 0))?;
    makiri.define_singleton_method("__borrowed?", function!(borrowed_p, 1))?;
    makiri.define_singleton_method("__debug_assertions?", function!(debug_assertions_p, 0))?;
    m_xml.define_singleton_method("__decode", function!(xml_decode, 1))?;

    Ok(())
}
