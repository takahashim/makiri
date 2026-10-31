//! [`Error`]: a Ruby exception to raise, or one that was raised.
//!
//! Nothing here raises until `Error::raise`, which only a method's
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
use crate::rb::global::RbConst;

/// The class of an exception not yet made: named, and read only when the
/// exception is raised (or asked [`Error::is_kind_of`]), by a caller that
/// holds the GVL. So making an [`Error`] reads no Ruby at all - not even a
/// C global - and needs no [`Ruby`] token: an edit's closure, which must run
/// no Ruby, can make one.
#[derive(Clone, Copy)]
pub enum ErrorClass {
    /// One of Ruby's own exception classes.
    Builtin(Builtin),
    /// A class the extension defines at `Init_` and registers;
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
    /// An exception of `class` with a message, made when it is raised.
    New(ErrorClass, Text),
    /// An exception object - one a protected C call caught.
    Exception(Value),
    /// A non-local exit a protected call caught that is not an exception -
    /// a `throw`, a `break` or `return` out of a block, a thread kill - by
    /// its `rb_protect` state, resumed as it was.
    Jump(c_int),
}

/// The message of an exception not yet made: fixed, or formatted into a
/// buffer held inline.
#[allow(
    clippy::large_enum_variant,
    reason = "a formatted message is held inline (`crate::msg::Msg`), so it is never boxed"
)]
enum Text {
    Fixed(&'static str),
    Formatted(crate::msg::Msg),
}

impl Text {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Text::Fixed(s) => s,
            Text::Formatted(m) => m.as_str(),
        }
    }

    /// The bytes, as the exception is made from them: no UTF-8 check, which
    /// the raise path - a `no_unwind` site - has no use for.
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        match self {
            Text::Fixed(s) => s.as_bytes(),
            Text::Formatted(m) => m.as_bytes(),
        }
    }
}

/// A message an [`Error`] can carry: a `&'static str`, or `format_args!`.
pub trait Message {
    fn into_error(self, class: ErrorClass) -> Error;
}

impl Message for &'static str {
    #[inline]
    fn into_error(self, class: ErrorClass) -> Error {
        Error(Kind::New(class, Text::Fixed(self)))
    }
}

impl Message for crate::msg::Msg {
    #[inline]
    fn into_error(self, class: ErrorClass) -> Error {
        Error(Kind::New(class, Text::Formatted(self)))
    }
}

impl Message for fmt::Arguments<'_> {
    fn into_error(self, class: ErrorClass) -> Error {
        match self.as_str() {
            Some(s) => Error(Kind::New(class, Text::Fixed(s))),
            None => Error(Kind::New(
                class,
                Text::Formatted(crate::msg::Msg::format(self)),
            )),
        }
    }
}

impl Error {
    /// An exception of `class` carrying `msg`.
    #[inline]
    pub fn new(class: impl Into<ErrorClass>, msg: impl Message) -> Error {
        msg.into_error(class.into())
    }

    /// The exception a protected call caught (`raw::protect`).
    #[inline]
    pub(crate) fn exception(exc: Value) -> Error {
        Error(Kind::Exception(exc))
    }

    /// The non-local exit `state` stands for.
    #[inline]
    pub(crate) fn jump(state: c_int) -> Error {
        Error(Kind::Jump(state))
    }

    /// An instance of `class`, or one that will be? `false` for an exit that
    /// is not an exception.
    pub fn is_kind_of(&self, ruby: &Ruby, class: impl Into<ErrorClass>) -> bool {
        let class = class.into().resolve(ruby);
        match &self.0 {
            Kind::New(c, _) => {
                crate::rb::object::class_inherits(c.resolve(ruby).as_value(), class.as_value())
            }
            Kind::Exception(v) => v.is_kind_of(class),
            Kind::Jump(_) => false,
        }
    }

    /// Handle this error, if it is an exception of one of `classes` (or of
    /// a class deriving from one): `Ok` with a [`Rescued`], which can be
    /// dropped. Anything else comes back as the `Err` it was - another
    /// exception, and every non-local exit (a `throw`, a `break`, a thread
    /// kill), which no class names - to be passed on with `?`.
    ///
    /// The way an `Err` becomes "no answer" (`rake dylint`'s
    /// `discarded_ruby_error` refuses the others; the one exception is
    /// [`crate::describe`]'s wording of a message), so a handler names what
    /// it handles, and an exit cannot be handled by mistake.
    pub fn rescue<C: Into<ErrorClass> + Copy>(
        self,
        ruby: &Ruby,
        classes: &[C],
    ) -> Result<Rescued, Error> {
        if classes.iter().any(|&c| self.is_kind_of(ruby, c)) {
            Ok(Rescued(()))
        } else {
            Err(self)
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
    ///
    /// An exit still pending ([`crate::raw::pending_exit`]) goes first,
    /// whatever this error is: the method dropped the exit's own `Err`, and
    /// a `throw` or a kill must not be lost for the error that followed it.
    #[inline]
    pub(crate) fn raise(self, ruby: &Ruby) -> ! {
        let mut e = crate::raw::pending_exit().unwrap_or(self);
        loop {
            let exc = match e.0 {
                Kind::Jump(state) => crate::raw::resume(state),
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
            };
            crate::raw::exc_raise(exc)
        }
    }
}

/// An exception [`Error::rescue`] handled: proof that what was dropped was
/// one of the classes its handler named, never a non-local exit. Carries
/// nothing; dropping it is the point.
pub struct Rescued(());

/// An [`Error`]'s message, as [`Error::wording`] writes it.
pub struct Wording<'a>(&'a Error, &'a Ruby);

impl fmt::Display for Wording<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 .0 {
            Kind::New(_, msg) => f.write_str(msg.as_str()),
            Kind::Exception(v) => crate::describe::ExceptionText(self.1, *v).fmt(f),
            Kind::Jump(_) => f.write_str("error"),
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Kind::New(_, msg) => write!(f, "Error({:?})", msg.as_str()),
            Kind::Exception(_) => f.write_str("Error(<exception>)"),
            Kind::Jump(state) => write!(f, "Error(<jump {state}>)"),
        }
    }
}

/* ---- the errors every layer makes ---- */

/// The internal error: the class a broken invariant of the layer raises, as
/// the extension registers it ([`RbConst::set`](crate::rb::global::RbConst::set)),
/// and `RuntimeError` until it does.
pub static INTERNAL_ERROR: RbConst<ExceptionClass> = RbConst::new();

/// An internal error carrying `msg`: a broken invariant, which a bare
/// `rescue` (StandardError) passes through - the registered class
/// ([`INTERNAL_ERROR`]), or `RuntimeError` before one is.
pub fn internal_error(msg: impl crate::rb::Message) -> Error {
    Error::new(&INTERNAL_ERROR, msg)
}

/// A `TypeError` with `msg`.
pub fn type_error(msg: impl crate::rb::Message) -> Error {
    Error::new(Builtin::TypeError, msg)
}

/// An `ArgumentError` with `msg`.
pub fn arg_error(msg: impl crate::rb::Message) -> Error {
    Error::new(Builtin::ArgumentError, msg)
}
