//! Ruby String <-> Makiri text.
//!
//! # Every String crosses as a copy
//!
//! A pointer *into* a Ruby String is valid only until Ruby runs: a GC can move
//! or free the backing buffer, and Ruby code can rewrite it (an argument's
//! `#to_s` could, while arguments were converted with it). The C held
//! such pointers and guarded them with a rule nobody could check; the first
//! Rust port held them under `rb_str_locktmp`, with a copy for a String that
//! was already locked, a frozen fast path and an interrupt to re-raise.
//!
//! All of that existed to spare a copy of a few bytes. The Strings that reach
//! the engine through this module are control strings - an XPath expression,
//! a selector, a name, an attribute value - and the one large input, the
//! HTML source, was copied out anyway before the GVL was released. So every
//! String is copied, in one place ([`copy_bytes`]), with no Ruby between the
//! read and the copy. A checked view ([`RubyText`], [`RubyData`]) then owns
//! its bytes: nothing can change them, so its `Deref` to `str` is plain. A
//! short copy stays on the stack ([`Bytes::Inline`]), so the hot paths - a
//! selector, an expression - allocate nothing.
//!
//! What a borrow still buys is a scan that needs no copy at all: the XML
//! reader's encoding sniff reads a prefix of a possibly large String before
//! deciding how to decode it. That is [`with_bytes`], the one scoped borrow,
//! whose closure must run no Ruby.

mod build;
mod encoding;
mod source;
mod view;

pub use build::*;
pub use encoding::*;
pub use source::*;
pub use view::*;
