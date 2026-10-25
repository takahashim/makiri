//! A Rust function as a Ruby method: the `extern "C"` trampoline Ruby calls,
//! and the one place a method's error is raised.
//!
//! `method!(f, n)` / `function!(f, n)` expand to a trampoline of arity `n`
//! (-1: Ruby passes `argc`/`argv`) that calls `f` through [`finish`]. `finish`
//! catches a panic - which becomes `Makiri::InternalError` - and, once `f`
//! and everything it owned have returned and dropped, raises the `Err` it
//! answered: the raise is a `longjmp` with nothing left to skip. The
//! expansion holds no `unsafe`, so it can stand in the `forbid(unsafe_code)`
//! glue: the trampoline is a safe `extern "C" fn`, and the C argument array
//! is a [`CArgv`], which only Ruby makes.
//!
//! `f` takes `&Ruby` first or not, then for a method its receiver, then its
//! arguments - each read by [`TryConvert`], which for Makiri means a `Value`
//! as it is or a receiver type checking its class, never a conversion that
//! runs Ruby - and returns anything [`ReturnValue`].

#![allow(unsafe_code)]

use core::ffi::c_int;
use core::panic::AssertUnwindSafe;

use super::convert::{ReturnValue, TryConvert};
use super::error::Error;
use super::ruby::Ruby;
use super::value::Value;

/// A trampoline and its arity, for `define_method` and its kin.
#[derive(Clone, Copy)]
pub struct Func {
    pub(in crate::nupur) ptr: unsafe extern "C" fn() -> rb_sys::VALUE,
    pub(in crate::nupur) arity: c_int,
}

/// Ruby's argument array for an arity -1 method: made only by Ruby, as the
/// second parameter of a trampoline, and good for that call.
#[repr(transparent)]
pub struct CArgv(*const Value);

/// Run a method's body: its `Ok` value is the method's result, its `Err` is
/// raised, a panic is `Makiri::InternalError`. The raise happens here, after
/// `f` has returned - every destructor of the method's frames has run - and
/// the error's own message has been released ([`Error::raise`]).
#[doc(hidden)]
#[inline]
pub fn finish(f: impl FnOnce() -> Result<Value, Error>) -> Value {
    crate::nupur::no_unwind::must_not_unwind::<crate::nupur::no_unwind::Method, _>(|| {
        let e = match std::panic::catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(v)) => return v,
            Ok(Err(e)) => e,
            Err(payload) => {
                let msg =
                    crate::nupur::caught::quietly(crate::nupur::msg::Msg::copy("a panic"), || {
                        crate::nupur::msg::Msg::copy(crate::nupur::caught::message(
                            payload.as_ref(),
                        ))
                    });
                crate::nupur::caught::quietly((), || drop(payload));
                crate::nupur::helpers::internal_error(msg)
            }
        };
        e.raise()
    })
}

/// [`finish`] for an arity -1 method: `f` is handed the arguments as a slice
/// of Ruby's array.
#[doc(hidden)]
#[inline]
pub fn finish_cary(
    argc: c_int,
    argv: CArgv,
    f: impl FnOnce(&[Value]) -> Result<Value, Error>,
) -> Value {
    let args: &[Value] = if argc <= 0 || argv.0.is_null() {
        &[]
    } else {
        // SAFETY: Ruby passes `argc` live values at `argv` for the duration of
        // the call, and the slice does not outlive it (`f` borrows it). A
        // `CArgv` exists only as the parameter Ruby filled.
        unsafe { core::slice::from_raw_parts(argv.0, argc as usize) }
    };
    finish(|| f(args))
}

impl Func {
    /// # Safety
    /// `f` is a trampoline Ruby may call with `arity` arguments as that
    /// arity's C signature.
    unsafe fn of<F: Copy>(f: F, arity: c_int) -> Func {
        const { assert!(core::mem::size_of::<F>() == core::mem::size_of::<fn()>()) };
        // SAFETY: a function pointer, cast to the generic type Ruby's
        // `rb_define_method` takes, which calls it with `arity`'s signature.
        Func {
            ptr: unsafe { core::mem::transmute_copy(&f) },
            arity,
        }
    }

    #[doc(hidden)]
    pub fn cary(f: extern "C" fn(c_int, CArgv, Value) -> Value) -> Func {
        // SAFETY: arity -1's signature: `(int argc, VALUE *argv, VALUE self)`.
        unsafe { Func::of(f, -1) }
    }
}

macro_rules! arity {
    ($n:literal, $new:ident, $method:ident, $ruby_method:ident, $function:ident, $ruby_function:ident;
     $($a:ident : $t:ident),*) => {
        impl Func {
            #[doc(hidden)]
            pub fn $new(f: extern "C" fn(Value $(, arity!(@v $a))*) -> Value) -> Func {
                // SAFETY: the signature of arity `$n`: the receiver, then `$n`
                // arguments, each a `VALUE` (`Value` is transparent over one).
                unsafe { Func::of(f, $n) }
            }
        }

        #[doc(hidden)]
        pub trait $method<S, $($t,)* R> {
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, S, $($t,)* R> $method<S, $($t,)* R> for F
        where
            F: Fn(S $(, $t)*) -> R,
            S: TryConvert,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)(S::try_convert(s)? $(, $t::try_convert($a)?)*).into_return()
            }
        }

        #[doc(hidden)]
        pub trait $ruby_method<S, $($t,)* R> {
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, S, $($t,)* R> $ruby_method<S, $($t,)* R> for F
        where
            F: for<'r> Fn(&'r Ruby, S $(, $t)*) -> R,
            S: TryConvert,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error> {
                // SAFETY: Ruby is calling this method: its thread, the GVL held.
                let ruby = unsafe { Ruby::get_unchecked() };
                (self)(&ruby, S::try_convert(s)? $(, $t::try_convert($a)?)*).into_return()
            }
        }

        #[doc(hidden)]
        pub trait $function<$($t,)* R> {
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, $($t,)* R> $function<$($t,)* R> for F
        where
            F: Fn($($t),*) -> R,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, _: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)($($t::try_convert($a)?),*).into_return()
            }
        }

        #[doc(hidden)]
        pub trait $ruby_function<$($t,)* R> {
            fn invoke(self, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, $($t,)* R> $ruby_function<$($t,)* R> for F
        where
            F: for<'r> Fn(&'r Ruby $(, $t)*) -> R,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, _: Value $(, $a: Value)*) -> Result<Value, Error> {
                // SAFETY: Ruby is calling this function: its thread, the GVL held.
                let ruby = unsafe { Ruby::get_unchecked() };
                (self)(&ruby $(, $t::try_convert($a)?)*).into_return()
            }
        }
    };
    (@v $a:ident) => { Value };
}

arity!(0, new0, Method0, RubyMethod0, Function0, RubyFunction0;);
arity!(1, new1, Method1, RubyMethod1, Function1, RubyFunction1; a0: A0);
arity!(2, new2, Method2, RubyMethod2, Function2, RubyFunction2; a0: A0, a1: A1);
arity!(3, new3, Method3, RubyMethod3, Function3, RubyFunction3; a0: A0, a1: A1, a2: A2);
arity!(4, new4, Method4, RubyMethod4, Function4, RubyFunction4; a0: A0, a1: A1, a2: A2, a3: A3);

/// Arity -1, with a receiver: `f(S, &[Value])`.
#[doc(hidden)]
pub trait MethodCAry<S, R> {
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, S, R> MethodCAry<S, R> for F
where
    F: for<'a> Fn(S, &'a [Value]) -> R,
    S: TryConvert,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(S::try_convert(s)?, args).into_return()
    }
}

/// Arity -1, with `&Ruby` and a receiver: `f(&Ruby, S, &[Value])`.
#[doc(hidden)]
pub trait RubyMethodCAry<S, R> {
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, S, R> RubyMethodCAry<S, R> for F
where
    F: for<'r, 'a> Fn(&'r Ruby, S, &'a [Value]) -> R,
    S: TryConvert,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error> {
        // SAFETY: Ruby is calling this method: its thread, the GVL held.
        let ruby = unsafe { Ruby::get_unchecked() };
        (self)(&ruby, S::try_convert(s)?, args).into_return()
    }
}

/// Arity -1, no receiver: `f(&[Value])`.
#[doc(hidden)]
pub trait FunctionCAry<R> {
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, R> FunctionCAry<R> for F
where
    F: for<'a> Fn(&'a [Value]) -> R,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, _: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(args).into_return()
    }
}

/// Arity -1, with `&Ruby` and no receiver: `f(&Ruby, &[Value])`.
#[doc(hidden)]
pub trait RubyFunctionCAry<R> {
    fn invoke(self, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, R> RubyFunctionCAry<R> for F
where
    F: for<'r, 'a> Fn(&'r Ruby, &'a [Value]) -> R,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, _: Value, args: &[Value]) -> Result<Value, Error> {
        // SAFETY: Ruby is calling this function: its thread, the GVL held.
        let ruby = unsafe { Ruby::get_unchecked() };
        (self)(&ruby, args).into_return()
    }
}

/// `method!(f, n)`: `f` as a Ruby method of arity `n` (-1 for `argc`/`argv`),
/// for `define_method`.
#[macro_export]
macro_rules! method {
    ($f:expr, -1) => {{
        extern "C" fn anon(
            argc: ::core::ffi::c_int,
            argv: $crate::nupur::rb::method::CArgv,
            rb_self: $crate::nupur::rb::Value,
        ) -> $crate::nupur::rb::Value {
            #[allow(unused_imports)]
            use $crate::nupur::rb::method::{MethodCAry as _, RubyMethodCAry as _};
            $crate::nupur::rb::method::finish_cary(argc, argv, |args| $f.invoke(rb_self, args))
        }
        $crate::nupur::rb::method::Func::cary(anon)
    }};
    ($f:expr, 0) => { $crate::__trampoline!(method $f, new0, Method0, RubyMethod0;) };
    ($f:expr, 1) => { $crate::__trampoline!(method $f, new1, Method1, RubyMethod1; a0) };
    ($f:expr, 2) => { $crate::__trampoline!(method $f, new2, Method2, RubyMethod2; a0, a1) };
    ($f:expr, 3) => { $crate::__trampoline!(method $f, new3, Method3, RubyMethod3; a0, a1, a2) };
    ($f:expr, 4) => { $crate::__trampoline!(method $f, new4, Method4, RubyMethod4; a0, a1, a2, a3) };
}

/// `function!(f, n)`: `f`, which takes no receiver, as a Ruby method of
/// arity `n` - a singleton method, say.
#[macro_export]
macro_rules! function {
    ($f:expr, -1) => {{
        extern "C" fn anon(
            argc: ::core::ffi::c_int,
            argv: $crate::nupur::rb::method::CArgv,
            rb_self: $crate::nupur::rb::Value,
        ) -> $crate::nupur::rb::Value {
            #[allow(unused_imports)]
            use $crate::nupur::rb::method::{FunctionCAry as _, RubyFunctionCAry as _};
            $crate::nupur::rb::method::finish_cary(argc, argv, |args| $f.invoke(rb_self, args))
        }
        $crate::nupur::rb::method::Func::cary(anon)
    }};
    ($f:expr, 0) => { $crate::__trampoline!(function $f, new0, Function0, RubyFunction0;) };
    ($f:expr, 1) => { $crate::__trampoline!(function $f, new1, Function1, RubyFunction1; a0) };
    ($f:expr, 2) => { $crate::__trampoline!(function $f, new2, Function2, RubyFunction2; a0, a1) };
    ($f:expr, 3) => { $crate::__trampoline!(function $f, new3, Function3, RubyFunction3; a0, a1, a2) };
    ($f:expr, 4) => { $crate::__trampoline!(function $f, new4, Function4, RubyFunction4; a0, a1, a2, a3) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trampoline {
    ($kind:ident $f:expr, $new:ident, $plain:ident, $ruby:ident; $($a:ident),*) => {{
        extern "C" fn anon(
            rb_self: $crate::nupur::rb::Value
            $(, $a: $crate::nupur::rb::Value)*
        ) -> $crate::nupur::rb::Value {
            #[allow(unused_imports)]
            use $crate::nupur::rb::method::{$plain as _, $ruby as _};
            $crate::nupur::rb::method::finish(|| $f.invoke(rb_self $(, $a)*))
        }
        $crate::nupur::rb::method::Func::$new(anon)
    }};
}

/// What `Init_makiri` runs `init` through: an `Err` is raised once `init`
/// has returned, and a panic is Ruby's `fatal` - `Makiri::InternalError` may
/// not exist yet, and an extension that stopped half-way through defining
/// itself must not be used, which `fatal` sees to.
#[doc(hidden)]
pub fn run_init(init: fn(&Ruby) -> Result<(), Error>) {
    crate::nupur::no_unwind::must_not_unwind::<crate::nupur::no_unwind::Init, _>(|| {
        // SAFETY: Ruby calls an extension's `Init_` on its thread, with the GVL.
        let ruby = unsafe { Ruby::get_unchecked() };
        let e = match std::panic::catch_unwind(AssertUnwindSafe(|| init(&ruby))) {
            Ok(Ok(())) => return,
            Ok(Err(e)) => e,
            Err(payload) => {
                let msg =
                    crate::nupur::caught::quietly(crate::nupur::msg::Msg::copy("a panic"), || {
                        crate::nupur::msg::Msg::copy(crate::nupur::caught::message(
                            payload.as_ref(),
                        ))
                    });
                crate::nupur::caught::quietly((), || drop(payload));
                match crate::nupur::raw::exception_new(
                    ruby.exception_runtime_error(),
                    msg.as_bytes(),
                ) {
                    Ok(exc) => crate::nupur::raw::exc_fatal(exc),
                    Err(e) => e,
                }
            }
        };
        e.raise()
    })
}
