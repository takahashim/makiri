//! Exception messages written without the heap.
//!
//! A message is formatted into [`Msg`], a fixed buffer kept inline - in a
//! `rb::Error`, which `Error::new` given `format_args!` makes - so an
//! error path allocates nothing of Rust's: a `format!` there would abort the
//! process under the very OOM it may be reporting. What a message quotes is written straight into the buffer -
//! bytes through [`Lossy`] and [`Quoted`], a Ruby value through `describe`'s
//! own `Display` types. Ruby-free.

#![forbid(unsafe_code)]

use core::fmt::{self, Write};

/// The longest message, in bytes. A longer one is cut at a character
/// boundary and ends with `...`: a message quotes untrusted input, and is
/// read by a person.
pub const MSG_MAX: usize = 256;

const ELLIPSIS: &str = "...";

/// A message being written: up to [`MSG_MAX`] bytes of UTF-8.
pub struct Msg {
    buf: [u8; MSG_MAX],
    len: u16,
    cut: bool,
}

impl Msg {
    const EMPTY: Msg = Msg {
        buf: [0; MSG_MAX],
        len: 0,
        cut: false,
    };

    /// `args`, written out.
    #[inline]
    pub fn format(args: fmt::Arguments<'_>) -> Msg {
        let mut m = Msg::EMPTY;
        /* A `Display` that fails leaves what it wrote; the message is still
         * worth raising. */
        let _ = m.write_fmt(args);
        m.finish()
    }

    /// `s`, copied - with no formatting machinery, so where nothing may
    /// unwind (a trampoline that has caught a panic, `crate::no_unwind`) it
    /// is provably panic-free.
    #[inline]
    pub fn copy(s: &str) -> Msg {
        let mut m = Msg::EMPTY;
        m.push(s);
        m.finish()
    }

    /// Mark a cut message as such.
    #[inline]
    fn finish(mut self) -> Msg {
        if self.cut {
            let len = floor_char_boundary(self.bytes(), MSG_MAX - ELLIPSIS.len());
            self.len = len as u16;
            self.cut = false;
            self.push(ELLIPSIS);
        }
        self
    }

    /// Append what fits of `s`, cut at a character boundary. No operation
    /// here can panic: every index is a checked `get`, and the arithmetic is
    /// on values below [`MSG_MAX`].
    #[inline]
    fn push(&mut self, s: &str) {
        let len = usize::from(self.len).min(MSG_MAX);
        let room = MSG_MAX.saturating_sub(len);
        let n = if s.len() <= room {
            s.len()
        } else {
            self.cut = true;
            floor_char_boundary(s.as_bytes(), room)
        };
        if let (Some(dst), Some(src)) = (self.buf.get_mut(len..), s.as_bytes().get(..n)) {
            for (d, b) in dst.iter_mut().zip(src) {
                *d = *b;
            }
            /* `len + n <= MSG_MAX`, which a u16 holds. */
            self.len = (len.saturating_add(n)).min(MSG_MAX) as u16;
        }
    }

    /// The bytes: UTF-8, as only whole `&str`s and their prefixes at a char
    /// boundary are ever copied in.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..usize::from(self.len)).unwrap_or(&[])
    }

    #[inline]
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }

    /// The text.
    #[inline]
    pub fn as_str(&self) -> &str {
        /* UTF-8 by construction (`as_bytes`); the fallback is unreachable. */
        core::str::from_utf8(self.bytes()).unwrap_or("error")
    }
}

/// The largest index `<= max` of `s` that starts a character (or ends `s`).
#[inline]
fn floor_char_boundary(s: &[u8], max: usize) -> usize {
    let mut i = max.min(s.len());
    while i > 0 && s.get(i).is_some_and(|&b| (b & 0xC0) == 0x80) {
        i -= 1;
    }
    i
}

impl Write for Msg {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if !self.cut {
            self.push(s);
        }
        Ok(())
    }
}

/// Bytes as text: invalid UTF-8 written as U+FFFD, as `from_utf8_lossy`
/// has it, with no copy.
pub struct Lossy<'a>(pub &'a [u8]);

impl fmt::Display for Lossy<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_char(char::REPLACEMENT_CHARACTER)?;
            }
        }
        Ok(())
    }
}

/// Bytes as a quoted, escaped string, as `{:?}` writes a `&str`; invalid
/// UTF-8 as U+FFFD.
pub struct Quoted<'a>(pub &'a [u8]);

impl fmt::Display for Quoted<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                match c {
                    '\'' => f.write_char('\'')?,
                    c => write!(f, "{}", c.escape_debug())?,
                }
            }
            if !chunk.invalid().is_empty() {
                f.write_char(char::REPLACEMENT_CHARACTER)?;
            }
        }
        f.write_char('"')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_that_fits_is_written_whole() {
        let m = Msg::format(format_args!("a {} c", "b"));
        assert_eq!(m.as_str(), "a b c");
    }

    #[test]
    fn a_long_message_is_cut_at_a_character_boundary() {
        let long = "é".repeat(MSG_MAX);
        let m = Msg::format(format_args!("x{long}"));
        let s = m.as_str();
        assert!(s.len() <= MSG_MAX);
        assert!(s.ends_with("..."));
        assert!(s.starts_with("xé"));
    }

    #[test]
    fn lossy_and_quoted_replace_invalid_bytes() {
        let m = Msg::format(format_args!(
            "{} {}",
            Lossy(b"a\xFFb"),
            Quoted(b"q\"\n\xFF")
        ));
        assert_eq!(m.as_str(), "a\u{FFFD}b \"q\\\"\\n\u{FFFD}\"");
    }
}
