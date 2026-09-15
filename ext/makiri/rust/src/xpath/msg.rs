//! Error messages: how the engine writes one, and how it quotes bytes into one.
//!
//! Separate from the C type declarations because an error path must not
//! allocate - one of them reports OOM - so every message is assembled in a
//! fixed stack buffer and truncated rather than grown.

use core::ffi::{c_char, c_int, CStr};

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
    pub status: c_int,
    msg: MsgBuf,
}

impl Error {
    /// No error yet: `XP_OK` and no message.
    pub fn new() -> Error {
        Error {
            status: XP_OK,
            msg: MsgBuf::default(),
        }
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

/// Replace `err`'s status and message. `msg` is copied, truncated to fit.
///
/// # Safety
/// `err` is null or a live error; `msg` is null or NUL-terminated.
#[cfg(feature = "lexbor")]
unsafe fn err_set_raw(err: *mut Error, status: c_int, msg: *const c_char) {
    if err.is_null() {
        return;
    }
    let e = &mut *err;
    e.status = status;
    e.msg.clear();
    if !msg.is_null() {
        e.msg.push_bytes(CStr::from_ptr(msg).to_bytes());
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

/// Where a failure is reported: the caller's error slot, or nowhere.
///
/// A copyable handle rather than a borrow, because every layer of the engine
/// passes it down beside its other raw handles. Null is spelled
/// [`ErrSink::silent`], so "don't tell me" is visible at the call site instead
/// of being one more `ptr::null_mut()` among the arguments.
#[derive(Clone, Copy)]
pub struct ErrSink(*mut Error);

impl ErrSink {
    /// Report into `slot`, which must outlive every use of the sink.
    pub fn new(slot: &mut Error) -> Self {
        ErrSink(slot)
    }

    /// Report nowhere: a failure still comes back as `Err(Reported)`, but no
    /// message is built.
    pub const fn silent() -> Self {
        ErrSink(core::ptr::null_mut())
    }

    pub fn is_silent(self) -> bool {
        self.0.is_null()
    }

    /// The slot, for a C-shaped callee.
    pub fn as_raw(self) -> *mut Error {
        self.0
    }
}

/// Set `err` from a formatted message, formatted straight into the slot.
///
/// Crate-internal, and the one place the front end writes an error. A silent
/// sink skips the formatting as well as the write.
pub(crate) fn err_set_fmt(err: ErrSink, status: c_int, args: core::fmt::Arguments<'_>) -> Reported {
    use core::fmt::Write;
    if !err.is_silent() {
        // SAFETY: a reporting sink names a live slot for every use.
        let e = unsafe { &mut *err.as_raw() };
        e.status = status;
        e.msg.clear();
        let _ = e.msg.write_fmt(args);
    }
    Reported(())
}

/// Set `err` to a fixed message: [`err_set_fmt`] without the formatting.
#[cfg(feature = "lexbor")]
pub(crate) fn err_set(err: ErrSink, status: c_int, msg: &core::ffi::CStr) -> Reported {
    // SAFETY: a reporting sink names a live slot for every use, as in
    // `err_set_fmt`, and `msg` is NUL-terminated.
    unsafe { err_set_raw(err.as_raw(), status, msg.as_ptr()) };
    Reported(())
}

/// `mkr_err_setf` for the Rust side: `err_setf!(err, status, "...", args)`.
#[macro_export]
macro_rules! err_setf {
    ($err:expr, $status:expr, $($arg:tt)*) => {
        $crate::xpath::msg::err_set_fmt($err, $status, format_args!($($arg)*))
    };
}

/* ---- statuses ---- */

pub const XP_OK: c_int = 0;
pub const XP_ERR_SYNTAX: c_int = 2;
pub const XP_ERR_INTERNAL: c_int = 5;
pub const XP_ERR_OOM: c_int = 6;
pub const XP_ERR_LIMIT: c_int = 7;
pub const XP_ERR_TYPE: c_int = 3;
pub const XP_ERR_RUNTIME: c_int = 4;
pub const XP_ERR_NOT_IMPLEMENTED: c_int = 1;
