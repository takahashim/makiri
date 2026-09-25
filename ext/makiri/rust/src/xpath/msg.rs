//! Error messages: how the engine writes one, and how it quotes bytes into one.
//!
//! An error path must not allocate - one of them reports OOM - so every message
//! is assembled in a fixed stack buffer and truncated rather than grown.

#![forbid(unsafe_code)]

use core::cell::RefCell;
use core::ffi::{c_char, CStr};
use std::rc::Rc;

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

    fn clear(&mut self) {
        self.len = 0;
        self.buf[0] = 0;
    }

    /// The message, or None when nothing was written.
    fn as_cstr(&self) -> Option<&CStr> {
        if self.len == 0 {
            return None;
        }
        CStr::from_bytes_until_nul(&self.buf).ok()
    }

    /// Append `b`, cut on a char boundary when it is UTF-8 and at the byte
    /// otherwise.
    #[cfg(feature = "lexbor")]
    fn push_bytes(&mut self, b: &[u8]) {
        use core::fmt::Write;
        match core::str::from_utf8(b) {
            Ok(s) => {
                let _ = self.write_str(s);
            }
            Err(_) => {
                let n = b.len().min(self.buf.len() - 1 - self.len);
                self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
                self.len += n;
                self.buf[self.len] = 0;
            }
        }
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
        self.buf[self.len] = 0;
        Ok(())
    }
}

/// An engine error: its status and its message, held inline.
///
/// No heap: the message is formatted into a fixed buffer and truncated rather
/// than grown, so every failure - running out of memory included - can say what
/// went wrong, and there is nothing to free afterwards.
pub struct Error {
    pub status: Status,
    msg: MsgBuf,
}

impl Error {
    /// An empty error slot: [`Status::Internal`] with no message.
    ///
    /// There is no "no error" status. A slot is only ever read after a failure
    /// has been reported (`Reported` proves it was written), so one read without
    /// being written is itself a broken invariant, and reads as one - never as
    /// a success, and never as a user's mistake.
    pub fn new() -> Error {
        Error {
            status: Status::Internal,
            msg: MsgBuf::default(),
        }
    }

    /// An error of `status`, its message formatted from `args` - for a caller
    /// that has an error to hand back and no run to report it through.
    pub fn with(status: Status, args: core::fmt::Arguments<'_>) -> Error {
        use core::fmt::Write;
        let mut e = Error::new();
        e.status = status;
        let _ = e.msg.write_fmt(args);
        e
    }

    /// The message, or None when none was written.
    pub fn message(&self) -> Option<&CStr> {
        self.msg.as_cstr()
    }
}

impl Default for Error {
    fn default() -> Self {
        Error::new()
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

/// Where a failure is reported: a handle to the run's error slot, or nowhere.
///
/// A cheaply copyable handle to a shared slot, because every layer of the engine
/// passes it down beside its other handles: a `Budget` keeps one and the CSS
/// lowering threads the same one through its builders, so the slot has to be
/// reachable from both without a borrow that would pin the whole budget. `None`
/// is [`ErrSink::silent`], so "don't tell me" is visible at the call site rather
/// than being one more null pointer among the arguments.
#[derive(Clone)]
pub struct ErrSink(Option<Rc<RefCell<Error>>>);

impl ErrSink {
    /// Report into `slot`.
    pub fn new(slot: Rc<RefCell<Error>>) -> Self {
        ErrSink(Some(slot))
    }

    /// Report nowhere: a failure still comes back as `Err(Reported)`, but no
    /// message is built.
    pub const fn silent() -> Self {
        ErrSink(None)
    }

    pub fn is_silent(&self) -> bool {
        self.0.is_none()
    }
}

/// Set `err` from a formatted message, formatted straight into the slot.
///
/// Crate-internal, and the one place the front end writes an error. A silent
/// sink skips the formatting as well as the write.
pub(crate) fn err_set_fmt(
    err: ErrSink,
    status: Status,
    args: core::fmt::Arguments<'_>,
) -> Reported {
    use core::fmt::Write;
    if let Some(slot) = err.0 {
        let mut e = slot.borrow_mut();
        e.status = status;
        e.msg.clear();
        let _ = e.msg.write_fmt(args);
    }
    Reported(())
}

/// Set `err` to a fixed message: [`err_set_fmt`] without the formatting.
#[cfg(feature = "lexbor")]
pub(crate) fn err_set(err: ErrSink, status: Status, msg: &CStr) -> Reported {
    if let Some(slot) = err.0 {
        let mut e = slot.borrow_mut();
        e.status = status;
        e.msg.clear();
        e.msg.push_bytes(msg.to_bytes());
    }
    Reported(())
}

/// Write a formatted error to a sink: `err_setf!(err, status, "...", args)`.
#[macro_export]
macro_rules! err_setf {
    ($err:expr, $status:expr, $($arg:tt)*) => {
        $crate::xpath::msg::err_set_fmt(
            ::core::clone::Clone::clone(&$err),
            $status,
            format_args!($($arg)*),
        )
    };
}

/* ---- statuses ---- */

/// What kind of failure an [`Error`] is. The Ruby layer picks the exception
/// class from it, with a `match` the compiler checks is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// A construct the engine does not implement (the namespace axis).
    NotImplemented,
    /// The expression does not parse.
    Syntax,
    /// A value of the wrong type (a predicate over a number).
    Type,
    /// A failure while evaluating (an unknown function, prefix or variable).
    Runtime,
    /// A broken invariant.
    Internal,
    /// Out of memory.
    Oom,
    /// A budget or cap was exceeded.
    Limit,
}
