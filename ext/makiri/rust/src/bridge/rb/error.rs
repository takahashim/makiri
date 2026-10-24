//! [`Error`]: a Ruby exception to raise, or one that was raised.
//!
//! Nothing here raises until [`Error::raise`], which only a method's
//! trampoline calls, once every Rust frame of the method has returned. Until
//! then an error is an ordinary value, returned up through `?` with every
//! destructor on the way run.

use std::borrow::Cow;

use core::ffi::c_int;

use super::value::{ExceptionClass, ReprValue, Value};

/// What went wrong, as Ruby will see it.
pub struct Error(Kind);

enum Kind {
    /// An exception of `class` with a message, made when it is raised.
    New(ExceptionClass, Cow<'static, str>),
    /// An exception object - one a protected C call caught.
    Exception(Value),
    /// A non-local exit a protected call caught that is not an exception -
    /// a `throw`, a `break` or `return` out of a block, a thread kill - by
    /// its `rb_protect` state, resumed as it was.
    Jump(c_int),
}

impl Error {
    /// An exception of `class` carrying `msg`.
    #[inline]
    pub fn new(class: ExceptionClass, msg: impl Into<Cow<'static, str>>) -> Error {
        Error(Kind::New(class, msg.into()))
    }

    /// The error a non-zero `rb_protect` state stands for: the exception
    /// Ruby left in `$!` for a raise (taken, so `$!` is clear again), the
    /// state itself for any other exit.
    pub(in crate::bridge) fn from_state(state: c_int) -> Error {
        if state != crate::bridge::raw::TAG_RAISE {
            return Error(Kind::Jump(state));
        }
        let exc = crate::bridge::raw::take_errinfo();
        if exc.is_nil() {
            return Error(Kind::Jump(state));
        }
        Error(Kind::Exception(exc))
    }

    /// An instance of `class`, or one that will be? `false` for an exit that
    /// is not an exception.
    pub fn is_kind_of(&self, class: ExceptionClass) -> bool {
        match &self.0 {
            Kind::New(c, _) => crate::bridge::raw::class_inherits(c.as_value(), class.as_value()),
            Kind::Exception(v) => v.is_kind_of(class),
            Kind::Jump(_) => false,
        }
    }

    /// The message, for wording another error with it - "error" for an exit
    /// that carries none. Reads an exception's message without calling it.
    pub fn message_for_wording(&self) -> String {
        match &self.0 {
            Kind::New(_, msg) => msg.clone().into_owned(),
            Kind::Exception(v) => crate::bridge::ruby::exception_message(*v),
            Kind::Jump(_) => "error".to_owned(),
        }
    }

    /// Raise it. Only a method's trampoline calls this, as its last act:
    /// the raise is a `longjmp` over every frame up to Ruby's, which runs no
    /// destructor, so nothing may be owned on the way. The message is the
    /// last thing owned here, and is released before the jump.
    pub(in crate::bridge) fn raise(self) -> ! {
        let mut e = self;
        loop {
            let exc = match e.0 {
                Kind::Jump(state) => crate::bridge::raw::jump_tag(state),
                Kind::Exception(v) => v,
                Kind::New(class, msg) => {
                    /* Made under `rb_protect`: the String and the object are
                     * allocations, and an exception class's `initialize` is
                     * Ruby's. A failure there is raised in its place. */
                    let made = crate::bridge::raw::exception_new(class, msg.as_bytes());
                    drop(msg);
                    match made {
                        Ok(v) => v,
                        Err(again) => {
                            e = again;
                            continue;
                        }
                    }
                }
            };
            crate::bridge::raw::exc_raise(exc)
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Kind::New(_, msg) => write!(f, "Error({msg:?})"),
            Kind::Exception(_) => f.write_str("Error(<exception>)"),
            Kind::Jump(state) => write!(f, "Error(<jump {state}>)"),
        }
    }
}
