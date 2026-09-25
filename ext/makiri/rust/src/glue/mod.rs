//! The Ruby surface: every Ruby method Makiri defines that is not simply a
//! TypedData primitive, with its argument reading, its errors and its
//! registration.
//!
//! The layers around it: `bridge` is the unsafe seam - raw Ruby strings, the
//! TypedData wrappers, `rb_protect` - and hands this layer safe primitives;
//! `lexbor`, `xml`, `xpath` and `css` are Ruby-free engines that take bytes and
//! return their own error types. So this layer holds no unsafe, which the
//! `forbid` below and `rake unsafe:boundaries` both pin.
//!
//! # Two rules this layer lives by
//!
//! **Never let C longjmp through a Rust frame.** `rb_raise` unwinds with
//! `longjmp`, which skips Rust destructors: a raise crossing a frame that owns a
//! `Vec`, a [`Buf`](crate::cbuf::Buf), a `RefCell` borrow or any other resource
//! leaks it or leaves it held. So a failure travels back as
//! `Result<_, magnus::Error>`, which magnus turns into a raise after the Rust
//! frames have returned normally, and a Ruby C function that can raise is called
//! through `bridge::ruby`, which catches the raise and hands it back the same
//! way. `rake unsafe:boundaries` holds the count of raising C calls outside
//! `bridge/` at zero. What can still unwind is Ruby itself - its allocator's
//! `NoMemoryError`, or Ruby code a C call reaches - which is why the loops that
//! push into a NodeSet while owning a collection run under `protect`.
//!
//! **Nothing Ruby crosses into a GVL-released closure.** Not a `Value`, not a
//! `Ruby` handle: `bridge::gvl::without_gvl` takes only a `Send` body, and a
//! parse copies its input out of Ruby before it releases the lock.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, RHash, Ruby, Value};

/// The keyword Hash of a method that takes only keywords, or `None` for a call
/// with no arguments.
///
/// The no-argument call returns before `scan_args`. It is the overwhelmingly
/// common one - `to_html` with a keyword is the exception - and routing it
/// through `scan_args` cost about a quarter of the per-call throughput on a
/// small element, which is all such a method does at that size.
pub(crate) fn keywords(args: &[Value]) -> Result<Option<RHash>, Error> {
    if args.is_empty() {
        return Ok(None);
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    Ok(Some(scanned.keywords))
}

/// Keyword `name`, read for truthiness: absent and `nil` are false, any other
/// value - `0` included, this being Ruby - true. A plain lookup rather than
/// `get_kwargs`, which allocates a second hash for the keys it was not asked
/// about, and so ignores unknown keywords.
pub(crate) fn kw_flag(ruby: &Ruby, kw: Option<RHash>, name: &str) -> bool {
    kw.and_then(|h| h.get(ruby.sym_new(name)))
        .is_some_and(|v: Value| v.to_bool())
}

/// `Makiri::Lexbor::CSS.parse_stylesheet` - the Ruby half of `lexbor::stylesheet`.
pub mod stylesheet;

/// What a rejected CSS selector raises, for both representations.
pub mod css;

/// `Makiri::HTML::Document` and the HTML fragment entry points.
pub mod html_doc;

/// The identity methods (`==`, `hash`, `pointer_id`) both representations share.
pub mod node;

/// `Makiri::NodeSet`.
pub mod node_set;

/// `#xpath` / `#at_xpath` for both representations, and the query path.
pub mod query;

/// `Makiri::XPathContext`.
pub mod xpath_context;

/// `Makiri::XML::Document` and `DocumentFragment`.
pub mod xml_doc;

/// The `Makiri::XML::*` node surface.
pub mod xml_node;

/// The `Makiri::HTML::*` node surface.
pub mod html_node;
