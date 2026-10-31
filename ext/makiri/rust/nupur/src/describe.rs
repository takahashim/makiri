//! Ruby values written into a message, without a call: `#inspect` and
//! `#to_s` would be the caller's Ruby code, and a message is made on an
//! error path, where nothing may raise.
//!
//! A String's bytes are written from a copy (`Head`), never from a slice
//! of the String: a `Display` writes into whatever `fmt::Write` its caller
//! chose, which this module cannot hold to running no Ruby.

#![forbid(unsafe_code)]

use crate::msg::{floor_char_boundary, Lossy, Quoted, MSG_MAX};
use crate::rb::{prelude::*, Error, RString, Ruby, Value};

/// A lookup made only to word a message - a Symbol's name, a class's name,
/// an ID - with `$!` kept: its value, or `None` for the message to write a
/// fallback in its place.
///
/// The one place the layer drops a Ruby error, and why that is safe: these
/// calls run no Ruby code and deliver no interrupt, so what can fail is an
/// allocation (`NoMemoryError`), which a message answers with a fallback, or
/// the exit already pending (a `throw`, a `break`, a kill), which every
/// protected call answers while one is and the trampoline resumes however
/// the method ends. Anything that runs Ruby goes through `?` or
/// [`Error::rescue`] instead.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        discarded_ruby_error,
        reason = "a message's lookup: an allocation failure is written as a fallback, and a pending exit is the trampoline's"
    )
)]
pub(crate) fn for_message<T>(f: impl FnOnce() -> Result<T, Error>) -> Option<T> {
    crate::raw::keeping_errinfo(f).ok()
}

/// Room for a message's worth of a String and the start of the character
/// that runs past it, so the cut can be put at a character boundary.
const HEAD_ROOM: usize = MSG_MAX + 4;

/// The first [`MSG_MAX`] bytes of a String, copied onto the stack: all of a
/// message there is room for, since [`crate::msg::Msg`] keeps no more.
/// `cut` says the String went on, so a message says it was cut even where
/// the copy alone would fit.
struct Head {
    buf: [u8; HEAD_ROOM],
    len: usize,
    cut: bool,
}

impl Head {
    fn of(s: RString) -> Head {
        let mut buf = [0u8; HEAD_ROOM];
        /* Copied, and its length read, with nothing run between. */
        let n = s.copy_to(0, &mut buf);
        let cut = n > MSG_MAX || s.len() > n;
        let len = if cut {
            floor_char_boundary(buf.get(..n).unwrap_or_default(), MSG_MAX)
        } else {
            n
        };
        Head { buf, len, cut }
    }

    fn bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// `...` after a cut String, as [`crate::msg::Msg`] ends a cut message.
    fn mark_cut(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.cut {
            f.write_str("...")?;
        }
        Ok(())
    }
}

/// `v` for an error message, written without calling it: a Symbol as
/// `:name`, a String quoted, anything else by its class (`#inspect` would be
/// the caller's Ruby).
pub struct Describe(pub Value);

impl core::fmt::Display for Describe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.0;
        if let Some(sym) = crate::rb::Symbol::from_value(v) {
            /* A name that cannot be made is written by the class instead. */
            if let Some(s) = for_message(|| super::raw::sym2str(sym)) {
                let head = Head::of(s);
                write!(f, ":{}", Lossy(head.bytes()))?;
                return head.mark_cut(f);
            }
        }
        if let Some(s) = RString::from_value(v) {
            let head = Head::of(s);
            Quoted(head.bytes()).fmt(f)?;
            return head.mark_cut(f);
        }
        if v.is_nil() {
            return f.write_str("nil");
        }
        f.write_str("an instance of ")?;
        v.write_classname(f)
    }
}

/// `v` as text for a message, as Ruby's `to_s` would have it for the values a
/// message holds - a String's text, a Symbol's name - but without calling it
/// (`to_s` is the caller's Ruby); anything else as [`Describe`] has it.
pub struct AsText(pub Value);

impl core::fmt::Display for AsText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.0;
        /* The String or the Symbol's name, read directly: `name_arg` would
         * build (and format) a whole TypeError for any other value, only for
         * it to be thrown away here. */
        let text = match crate::rb::Symbol::from_value(v) {
            Some(sym) => for_message(|| super::raw::sym2str(sym)),
            None => match RString::from_value(v) {
                Some(s) => Some(s),
                None => return Describe(v).fmt(f),
            },
        };
        if let Some(s) = text {
            let head = Head::of(s);
            Lossy(head.bytes()).fmt(f)?;
            return head.mark_cut(f);
        }
        Describe(v).fmt(f)
    }
}

/// The class name of a value, for a message.
pub(crate) struct ClassName(pub(crate) Value);

impl core::fmt::Display for ClassName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.write_classname(f)
    }
}

/// The longest message kept: this runs on error paths to word another error,
/// and an exception's message is untrusted input.
const EXCEPTION_MESSAGE_MAX: usize = 255;

/// `exc`'s message, for wording another error with it: up to the first NUL,
/// at most `EXCEPTION_MESSAGE_MAX` bytes, invalid UTF-8 replaced. "error"
/// for a message that is not a String - this runs on error paths, so it must
/// not raise itself.
pub struct ExceptionText<'a>(pub &'a Ruby, pub Value);

impl core::fmt::Display for ExceptionText<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        /* The message is the exception's `mesg`, read as an attribute: calling
         * `#message` would run its class's Ruby. A message that is not a
         * String is answered as "error". */
        let Some(id) = for_message(|| crate::rb::HiddenId::intern(self.0, c"mesg")) else {
            return f.write_str("error");
        };
        /* An attribute read: it runs no Ruby and raises nothing. */
        let Some(r) = RString::from_value(crate::rb::object::ivar_get(self.1, id)) else {
            return f.write_str("error");
        };
        let head = Head::of(r);
        let bytes = head.bytes();
        let n = bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(bytes.len())
            .min(EXCEPTION_MESSAGE_MAX);
        Lossy(bytes.get(..n).unwrap_or_default()).fmt(f)
    }
}
