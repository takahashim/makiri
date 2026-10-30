//! A Ruby String's bytes, read and written by copying: the layer hands no
//! slice of a String out.
//!
//! Reading (`read`) is [`RString::copy_to`](crate::rb::RString::copy_to)
//! and [`RString::append_to`](crate::rb::RString::append_to), each a read and
//! a copy with nothing run between, so nothing Ruby does afterwards - a GC,
//! the String rewritten or grown - reaches what the caller holds; as text,
//! [`read_utf8`]. Making one (`make`) is a copy, or a String written in
//! place front to back through an [`Unwritten`].

mod make;
mod read;

pub use make::{interned_utf8, utf8_str, utf8_str_filled, utf8_str_from_slices, Unwritten};
pub use read::{read_utf8, read_utf8_within, to_utf8, NoRoom, ReadError, Transcode};
