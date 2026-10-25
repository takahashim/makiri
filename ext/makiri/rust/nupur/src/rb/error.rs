//! [`Error`]: a Ruby exception to raise, or one that was raised.
//!
//! Nothing here raises until [`Error::raise`], which only a method's
//! trampoline calls, once every Rust frame of the method has returned. Until
//! then an error is an ordinary value, returned up through `?` with every
//! destructor on the way run.
//!
//! Nothing here allocates either, of Rust's or of Ruby's, until then: a fixed
//! message is a `&'static str`, a formatted one (`format_args!`) is written
//! into a buffer the error holds inline (`crate::msg::Msg`), and the exception
//! is made from either as it is raised, under `rb_protect` - so a failure to
//! make it is Ruby's `NoMemoryError`, never an abort. An error can be made
//! anywhere, an edit's closure that must run no Ruby included.

#![forbid(unsafe_code)]

use core::ffi::c_int;
use core::fmt;

use super::ruby::Ruby;
use super::value::{ExceptionClass, ReprValue, Value};
use crate::helpers::RbConst;

/// The class of an exception not yet made: named, and read only when the
/// exception is raised (or asked [`Error::is_kind_of`]), by a caller that
/// holds the GVL. So making an [`Error`] reads no Ruby at all - not even a
/// C global - and needs no [`Ruby`] token: an edit's closure, which must run
/// no Ruby, can make one.
#[derive(Clone, Copy)]
pub enum ErrorClass {
    /// One of Ruby's own exception classes.
    Builtin(Builtin),
    /// A class `init` defines and registers (`Makiri::Error`, ...);
    /// `RuntimeError` if it is read before it is set.
    Registered(&'static RbConst<ExceptionClass>),
    /// A class value a caller holding the GVL already has.
    Value(ExceptionClass),
}

/// Ruby's own exception classes an [`Error`] can name without a token.
#[derive(Clone, Copy)]
pub enum Builtin {
    RuntimeError,
    ArgumentError,
    TypeError,
    RangeError,
    NotImplementedError,
    NoMemoryError,
    EncodingError,
    LoadError,
}

impl ErrorClass {
    /// The class, read with the GVL held.
    #[inline]
    pub fn resolve(self, ruby: &Ruby) -> ExceptionClass {
        match self {
            ErrorClass::Builtin(b) => match b {
                Builtin::RuntimeError => ruby.exception_runtime_error(),
                Builtin::ArgumentError => ruby.exception_arg_error(),
                Builtin::TypeError => ruby.exception_type_error(),
                Builtin::RangeError => ruby.exception_range_error(),
                Builtin::NotImplementedError => ruby.exception_not_imp_error(),
                Builtin::NoMemoryError => ruby.exception_no_mem_error(),
                Builtin::EncodingError => ruby.exception_encoding_error(),
                Builtin::LoadError => ruby.exception_load_error(),
            },
            ErrorClass::Registered(c) => c.exception(ruby),
            ErrorClass::Value(c) => c,
        }
    }
}

impl From<Builtin> for ErrorClass {
    #[inline]
    fn from(b: Builtin) -> ErrorClass {
        ErrorClass::Builtin(b)
    }
}

impl From<&'static RbConst<ExceptionClass>> for ErrorClass {
    #[inline]
    fn from(c: &'static RbConst<ExceptionClass>) -> ErrorClass {
        ErrorClass::Registered(c)
    }
}

impl From<ExceptionClass> for ErrorClass {
    #[inline]
    fn from(c: ExceptionClass) -> ErrorClass {
        ErrorClass::Value(c)
    }
}

/// What went wrong, as Ruby will see it.
pub struct Error(Kind);

#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
enum Kind {
    /// An exception of `class` with a fixed message, made when it is raised.
    New(ErrorClass, &'static str),
    /// An exception of `class` with a formatted message, made when it is
    /// raised.
    Formatted(ErrorClass, crate::msg::Msg),
    /// An exception object - one a protected C call caught.
    Exception(Value),
    /// A non-local exit a protected call caught that is not an exception -
    /// a `throw`, a `break` or `return` out of a block, a thread kill - by
    /// its `rb_protect` state, resumed as it was.
    Jump(c_int),
}

/// A message an [`Error`] can carry: a `&'static str`, or `format_args!`.
pub trait Message {
    fn into_error(self, class: ErrorClass) -> Error;
}

impl Message for &'static str {
    #[inline]
    fn into_error(self, class: ErrorClass) -> Error {
        Error(Kind::New(class, self))
    }
}

impl Message for crate::msg::Msg {
    #[inline]
    fn into_error(self, class: ErrorClass) -> Error {
        Error(Kind::Formatted(class, self))
    }
}

impl Message for fmt::Arguments<'_> {
    fn into_error(self, class: ErrorClass) -> Error {
        match self.as_str() {
            Some(s) => Error(Kind::New(class, s)),
            None => Error(Kind::Formatted(class, crate::msg::Msg::format(self))),
        }
    }
}

impl Error {
    /// An exception of `class` carrying `msg`.
    #[inline]
    pub fn new(class: impl Into<ErrorClass>, msg: impl Message) -> Error {
        msg.into_error(class.into())
    }

    /// The error a non-zero `rb_protect` state stands for: the exception
    /// Ruby left in `$!` for a raise (taken, so `$!` is clear again), the
    /// state itself for any other exit.
    #[inline]
    pub(crate) fn from_state(state: c_int) -> Error {
        if state != crate::raw::TAG_RAISE {
            return Error(Kind::Jump(state));
        }
        let exc = crate::raw::take_errinfo();
        if exc.is_nil() {
            return Error(Kind::Jump(state));
        }
        Error(Kind::Exception(exc))
    }

    /// An instance of `class`, or one that will be? `false` for an exit that
    /// is not an exception.
    pub fn is_kind_of(&self, ruby: &Ruby, class: impl Into<ErrorClass>) -> bool {
        let class = class.into().resolve(ruby);
        match &self.0 {
            Kind::New(c, _) | Kind::Formatted(c, _) => {
                crate::raw::class_inherits(c.resolve(ruby).as_value(), class.as_value())
            }
            Kind::Exception(v) => v.is_kind_of(class),
            Kind::Jump(_) => false,
        }
    }

    /// The message, for wording another error with it - "error" for an exit
    /// that carries none. Reads an exception's message without calling it,
    /// as it is written out.
    pub fn wording<'a>(&'a self, ruby: &'a Ruby) -> Wording<'a> {
        Wording(self, ruby)
    }

    /// Raise it. Only a method's trampoline calls this, as its last act:
    /// the raise is a `longjmp` over every frame up to Ruby's, which runs no
    /// destructor, so nothing may be owned on the way - and nothing is.
    #[inline]
    pub(crate) fn raise(self, ruby: &Ruby) -> ! {
        let mut e = self;
        loop {
            let exc = match e.0 {
                Kind::Jump(state) => crate::raw::jump_tag(state),
                Kind::Exception(v) => v,
                /* Made under `rb_protect`: the String and the object are
                 * allocations, and an exception class's `initialize` is
                 * Ruby's. A failure there is raised in its place. */
                Kind::New(class, msg) => {
                    match crate::raw::exception_new(ruby, class.resolve(ruby), msg.as_bytes()) {
                        Ok(v) => v,
                        Err(again) => {
                            e = again;
                            continue;
                        }
                    }
                }
                Kind::Formatted(class, msg) => {
                    match crate::raw::exception_new(ruby, class.resolve(ruby), msg.as_bytes()) {
                        Ok(v) => v,
                        Err(again) => {
                            e = again;
                            continue;
                        }
                    }
                }
            };
            crate::raw::exc_raise(exc)
        }
    }
}

/// An [`Error`]'s message, as [`Error::wording`] writes it.
pub struct Wording<'a>(&'a Error, &'a Ruby);

impl fmt::Display for Wording<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 .0 {
            Kind::New(_, msg) => f.write_str(msg),
            Kind::Formatted(_, msg) => f.write_str(msg.as_str()),
            Kind::Exception(v) => crate::helpers::ExceptionText(self.1, *v).fmt(f),
            Kind::Jump(_) => f.write_str("error"),
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Kind::New(_, msg) => write!(f, "Error({msg:?})"),
            Kind::Formatted(_, msg) => write!(f, "Error({:?})", msg.as_str()),
            Kind::Exception(_) => f.write_str("Error(<exception>)"),
            Kind::Jump(state) => write!(f, "Error(<jump {state}>)"),
        }
    }
}
