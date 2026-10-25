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
//! `Result<_, crate::bridge::rb::Error>`, which the method's trampoline turns
//! into a raise after the Rust frames have returned normally
//! (`bridge::rb::method::finish`), and a Ruby C function that can raise is called
//! only through `bridge::raw`, whose every wrapper catches the raise and hands
//! it back the same way - Ruby's allocator's `NoMemoryError` included, so
//! the glue builds its Arrays, Hashes and Strings through `bridge::build` and
//! `bridge::string`. `rake dylint`'s `raising_call_outside_protect` holds the
//! raising C calls to `nupur::raw` (`bridge::raw`), each inside its protected
//! `call` - and the extension cannot name one at all, not depending on rb-sys.
//!
//! **Nothing Ruby crosses into a GVL-released closure.** Not a `Value`, not a
//! `Ruby` handle: `bridge::gvl::without_gvl` takes only a `Send` body, and a
//! parse copies its input out of Ruby before it releases the lock.

#![forbid(unsafe_code)]

/// A method's positional and keyword arguments, separated as `rb_scan_args`
/// separates them.
pub mod args;

/// Keyword arguments, read one way for every method.
pub mod kwargs;

/// Hash argument pairs, copied out before any caller code converts them.
pub mod hash;

/// `Makiri::Lexbor::CSS.parse_stylesheet` - the Ruby half of `selectors::stylesheet`.
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
