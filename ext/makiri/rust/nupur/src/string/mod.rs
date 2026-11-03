//! A Ruby String's bytes, read and written by copying - but for two loans
//! of its bytes while nothing can change them.
//!
//! Reading (`read`) is [`RString::copy_to`](crate::rb::RString::copy_to)
//! and [`RString::append_to`](crate::rb::RString::append_to), each a read and
//! a copy with nothing run between, so nothing Ruby does afterwards - a GC,
//! the String rewritten or grown - reaches what the caller holds; as text,
//! [`read_utf8`], as bytes whatever the encoding, [`read_bytes`]. The loans
//! hand a slice to a closure for the length of its call: `with_bytes`'s
//! closure can touch no Ruby at all, and `with_locked_bytes`'s String is
//! frozen or locked until it returns. Making one
//! (`make`) is a copy - of text, marked valid ([`utf8_str`]), of bytes the
//! caller holds to be UTF-8 ([`utf8_str_trusting`]), or ASCII-8BIT
//! ([`binary_str`]) - or a String written in place front to back through an
//! [`Unwritten`].

mod make;
mod read;

pub use make::{
    binary_str, interned_utf8_trusting, utf8_str, utf8_str_filled, utf8_str_from_slices,
    utf8_str_trusting, Unwritten,
};
pub use read::{read_bytes, read_utf8, read_utf8_within, to_utf8, NoRoom, ReadError, Transcode};
