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

/// `Makiri::Lexbor::CSS.parse_stylesheet` - the Ruby half of `lexbor::stylesheet`.
pub mod stylesheet;

/// `Makiri::HTML::Document` and the HTML fragment entry points.
pub mod html_doc;

pub mod node;

pub mod node_set;

/// `#xpath` / `#at_xpath` for both representations, and the query path.
pub mod query;

/// `Makiri::XPathContext`.
pub mod xpath_context;

/// `Makiri::XML::Document` and `DocumentFragment`.
pub mod xml_doc;

pub mod xml_node;

pub mod html_node;
