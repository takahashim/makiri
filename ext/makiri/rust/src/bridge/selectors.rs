//! `Node#css` / `#at_css` / `#matches?`.
//!
//! The selector engine, its process-global cache and its Lexbor callbacks live
//! in [`crate::lexbor::selectors`], which does not know about Ruby. This layer
//! verifies the selector, maps an engine failure to its exception, and fills the
//! NodeSet (`css`) or wraps the one node `at_css` found.

#![allow(unsafe_code)]

use crate::lexbor::adapter::html::RawNode;

use magnus::rb_sys::AsRawValue;

use crate::bridge::ruby::makiri_error;
use magnus::{method, prelude::*, Error, Ruby, Value};

use crate::bridge::gvl::held;
use crate::bridge::html::{html_node_unwrap, wrap_html_node};
use crate::bridge::node_set::{node_set_new, node_set_push, PushError};
use crate::bridge::ruby::VALUE;
use crate::bridge::string::{ruby_bytes_view, verify_text, RubyBytes};
use crate::bridge::wrapper::keepalive_document;
use crate::init::{EXC_CSS_SYNTAX_ERROR, MOD_HTML_NODE_METHODS};
use crate::lexbor::selectors::{matches_node, select_all, select_first, SelectError};

use crate::limits::NODE_SET_MAX;

/// An engine failure as the Ruby exception it maps to.
fn select_error(err: SelectError, selector: Value) -> Error {
    match err {
        SelectError::Syntax => {
            let class = magnus::ExceptionClass::from_value(EXC_CSS_SYNTAX_ERROR.value())
                .expect("Makiri::CSS::SyntaxError");
            /* `%" PRIsVALUE` interpolates a String with `to_s`, not `inspect`:
             * the C wrote the selector bare. */
            let shown = selector.to_string();
            Error::new(class, format!("invalid CSS selector: {shown}"))
        }
        SelectError::Overflow => makiri_error(format!(
            "CSS result set exceeded the node limit ({NODE_SET_MAX})"
        )),
        SelectError::CollectOom => makiri_error("out of memory collecting CSS results"),
        SelectError::CacheOom => makiri_error("out of memory caching CSS selector"),
        SelectError::Unavailable => makiri_error("failed to initialise CSS selector engine"),
        SelectError::Busy => makiri_error("CSS selector engine is already in use"),
    }
}

/// The selector's bytes, after the strict text check. `css` receives a String
/// already, so this reads its bytes without coercing.
#[inline]
fn selector_bytes(selector: Value) -> Result<RubyBytes, Error> {
    verify_text(selector, c"CSS selector")?;
    // SAFETY: `verify_text` accepted `selector` as a String, so it is a live
    // T_STRING whose bytes stay put for this call.
    Ok(unsafe { ruby_bytes_view(selector.as_raw()) })
}

/// The arguments to the fill loop, passed through `rb_protect`'s one
/// `VALUE`-sized slot.
struct Fill<'a> {
    set: VALUE,
    nodes: &'a [RawNode],
    /// A push the set refused, carried out of `rb_protect` for the caller.
    refused: Option<PushError>,
}

/// Move the collected matches into the NodeSet. Runs under `rb_protect`: a push
/// can raise (Ruby's allocator), and a longjmp straight out of here would skip
/// the collection Vec's drop in the caller.
/* A plain Rust fn, NOT `extern "C"`: it is only ever called from the
 * closure below, and that ABI would turn a panic in it into an abort
 * at its own boundary - before `bridge::ruby::protect`'s latch could
 * see it. Nothing passes it to C as a function pointer. */
unsafe fn fill_thunk(arg: VALUE) -> VALUE {
    let f = &mut *(arg as *mut Fill);
    for &n in f.nodes {
        if let Err(e) = node_set_push(f.set, n.as_ptr()) {
            f.refused = Some(e);
            break;
        }
    }
    crate::bridge::ruby::nil().as_raw()
}

/// `Node#css`: every matching descendant, in document order.
fn css(rb_self: Value, selector: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let root = html_node_unwrap(rb_self)?;
        let document = keepalive_document(rb_self)?;
        let sv = selector_bytes(selector)?;

        // SAFETY: the bytes are the verified view's, read for this call.
        let gvl = held(&Ruby::get_with(rb_self));
        let nodes =
            select_all(&gvl, root, unsafe { sv.bytes() }).map_err(|e| select_error(e, selector))?;

        let set = node_set_new(document);
        /* Each push can raise (NoMemoryError from Ruby's allocator), and a longjmp
         * would skip `nodes`'s drop. `protect` turns that into an Err, the Vec drops
         * on the way out, and magnus raises afterwards - the Rust form of the C's
         * rb_ensure, at one setjmp per call rather than per node. */
        let mut fill = Fill {
            set: set.as_raw(),
            nodes: &nodes,
            refused: None,
        };
        /* `protect` passes one machine word through, so the state travels as a
         * `*mut Fill` wearing a VALUE's type - never a Ruby object, and never
         * reached as one. */
        let fill_ptr = &mut fill as *mut Fill as VALUE;
        crate::bridge::ruby::protect_value(|| {
            // SAFETY: `fill_ptr` is the borrow above, which outlives the call, and
            // `fill_thunk` is the only reader - it casts the same word back.
            unsafe { fill_thunk(fill_ptr) }
        })?;
        if let Some(e) = fill.refused.take() {
            return Err(e.into());
        }
        Ok(set)
    })
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(rb_self: Value, selector: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let ruby = Ruby::get_with(rb_self);
        let root = html_node_unwrap(rb_self)?;
        let sv = selector_bytes(selector)?;

        // SAFETY: the bytes are the verified view's, read for this call.
        let found = select_first(&held(&ruby), root, unsafe { sv.bytes() })
            .map_err(|e| select_error(e, selector))?;
        let Some(node) = found else {
            return Ok(ruby.qnil().as_value());
        };
        let document = keepalive_document(rb_self)?;
        Ok(wrap_html_node(node, document))
    })
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri.
fn matches(rb_self: Value, selector: Value) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let root = html_node_unwrap(rb_self)?;
        let sv = selector_bytes(selector)?;
        // SAFETY: the bytes are the verified view's, read for this call.
        matches_node(&held(&Ruby::get_with(rb_self)), root, unsafe { sv.bytes() })
            .map_err(|e| select_error(e, selector))
    })
}

/// # Safety
/// Called from `Init_makiri`.
pub fn init_css() {
    let m = MOD_HTML_NODE_METHODS.module();
    m.define_method("css", method!(css, 1)).expect("Node#css");
    m.define_method("at_css", method!(at_css, 1))
        .expect("Node#at_css");
    m.define_method("matches?", method!(matches, 1))
        .expect("Node#matches?");
}
