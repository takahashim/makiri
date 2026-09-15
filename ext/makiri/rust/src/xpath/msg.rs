//! Error messages: how the engine writes one, and how it quotes bytes into one.
//!
//! Separate from the C type declarations because an error path must not
//! allocate - one of them reports OOM - so every message is assembled in a
//! fixed stack buffer and truncated rather than grown.

use super::abi::{mkr_err_set, Error};
use core::ffi::{c_char, c_int};

/// Bytes as text for a message, with anything non-ASCII-printable escaped, so a
/// name echoed back into an error cannot carry control bytes into the message.
pub struct Bytes<'a>(pub &'a [u8]);

impl core::fmt::Display for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for &b in self.0 {
            if (0x20..0x7F).contains(&b) {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{:02x}", b)?;
            }
        }
        Ok(())
    }
}

/// A NUL-terminated message assembled on the stack. Error paths must not
/// allocate - one of them reports OOM - so this is where messages are built,
/// and it truncates rather than growing.
pub struct MsgBuf {
    buf: [u8; 200],
    len: usize,
}

impl Default for MsgBuf {
    fn default() -> Self {
        MsgBuf {
            buf: [0; 200],
            len: 0,
        }
    }
}

impl MsgBuf {
    pub fn as_ptr(&self) -> *const c_char {
        self.buf.as_ptr() as *const c_char
    }
}

impl core::fmt::Write for MsgBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        /* Leave one byte for the terminator, and cut on a char boundary. */
        let room = self.buf.len() - 1 - self.len;
        let mut n = s.len().min(room);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Proof that an error has been written to the caller's error slot.
///
/// Only this module makes one, and only by writing the slot, so a
/// `Result<_, Reported>` cannot fail without its message having been set. A null
/// slot means the caller asked not to be told; the proof holds for it all the
/// same. Zero-sized, so a `Result<(), Reported>` costs what the `bool` or
/// `c_int` it replaces did - which matters on the per-node budget checks.
///
/// Public so public engine functions can return it; the field is private, so
/// nothing outside this module can make one without writing an error.
#[derive(Debug)]
pub struct Reported(());

/// Set `err` from a formatted message. `mkr_err_set` copies it (mkr_xpath.c),
/// so the stack buffer does not outlive the call.
///
/// Crate-internal, and the one place the front end writes an error: every
/// caller already holds the `*mut Error` its caller handed it, and passing a
/// NULL or a dangling one would be the caller's bug either way.
pub(crate) fn err_set_fmt(
    err: *mut Error,
    status: c_int,
    args: core::fmt::Arguments<'_>,
) -> Reported {
    use core::fmt::Write;
    let mut m = MsgBuf::default();
    let _ = m.write_fmt(args);
    unsafe { mkr_err_set(err, status, m.as_ptr()) };
    Reported(())
}

/// `mkr_err_setf` for the Rust side: `err_setf!(err, status, "...", args)`.
#[macro_export]
macro_rules! err_setf {
    ($err:expr, $status:expr, $($arg:tt)*) => {
        $crate::xpath::msg::err_set_fmt($err, $status, format_args!($($arg)*))
    };
}
