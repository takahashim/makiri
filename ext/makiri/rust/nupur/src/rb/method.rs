//! A Rust function as a Ruby method: the `extern "C"` trampoline Ruby calls,
//! and the one place a method's error is raised.
//!
//! `method!(f, n)` / `function!(f, n)` expand to a trampoline of arity `n`
//! (-1: Ruby passes `argc`/`argv`) that calls `f` through [`finish`]. `finish`
//! catches a panic - which becomes the internal error (`rb::error::INTERNAL_ERROR`) - and, once `f`
//! and everything it owned have returned and dropped, raises the `Err` it
//! answered: the raise is a `longjmp` with nothing left to skip. The
//! expansion's one `unsafe` block is its call of [`finish`] (or
//! [`finish_cary`]), the token's entry, which an external macro's expansion
//! may hold where the extension forbids `unsafe_code`; the caller's `f` is
//! bound outside that block, so an unsafe operation written in it needs an
//! `unsafe` of the caller's own. The C argument array is a [`CArgv`], which
//! only Ruby makes.
//!
//! `f` takes `&Ruby` first or not, then for a method its receiver, then its
//! arguments - each read by [`TryConvert`], which is a `Value`
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
    pub(crate) ptr: unsafe extern "C" fn() -> rb_sys::VALUE,
    pub(crate) arity: c_int,
}

/// Ruby's argument array for an arity -1 method: made only by Ruby, as the
/// second parameter of a trampoline, and good for that call.
#[repr(transparent)]
pub struct CArgv(*const Value);

/// Run a method's body: its `Ok` value is the method's result, its `Err` is
/// raised, a panic is the internal error (`rb::error::INTERNAL_ERROR`). The raise happens here, after
/// `f` has returned - every destructor of the method's frames has run - and
/// the error's own message has been released ([`Error::raise`]).
///
/// # Safety
/// Called only from a trampoline Ruby itself calls - the expansion of
/// `method!` / `function!` - so on a Ruby thread holding the GVL: this makes
/// the method's `Ruby` token, and a call from anywhere else (a thread
/// without the GVL) would make one that proves nothing. `unsafe` so that
/// nothing but the macros, whose expansion carries the `unsafe` block, can
/// call it: the extension's own code is `forbid(unsafe_code)`.
#[doc(hidden)]
#[inline]
pub unsafe fn finish(f: impl FnOnce(&Ruby) -> Result<Value, Error>) -> Value {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::Method, _>(|| {
        // SAFETY: only a trampoline Ruby calls runs this: its thread, the GVL
        // held. The one token a method's body sees is this one.
        let ruby = unsafe { Ruby::get_unchecked() };
        /* A method starts with no exit pending: every trampoline resumes its
         * own before it returns (below, and `Error::raise`), so this only
         * makes sure. */
        crate::raw::reset_pending_exit();
        let e = match std::panic::catch_unwind(AssertUnwindSafe(|| f(&ruby))) {
            /* An exit the method caught and dropped is resumed, not lost:
             * a `throw`'s catcher is waiting, a killed thread must end. */
            Ok(Ok(v)) => match crate::raw::pending_exit() {
                None => return v,
                Some(exit) => exit,
            },
            Ok(Err(e)) => e,
            Err(payload) => crate::rb::error::internal_error(crate::caught::panic_message(payload)),
        };
        e.raise(&ruby)
    })
}

/// [`finish`] for an arity -1 method: `f` is handed the arguments as a slice
/// of Ruby's array.
///
/// # Safety
/// As [`finish`]; and `argc` / `argv` are the arguments Ruby passed.
#[doc(hidden)]
#[inline]
pub unsafe fn finish_cary(
    argc: c_int,
    argv: CArgv,
    f: impl FnOnce(&Ruby, &[Value]) -> Result<Value, Error>,
) -> Value {
    let args: &[Value] = if argc <= 0 || argv.0.is_null() {
        &[]
    } else {
        // SAFETY: Ruby passes `argc` live values at `argv` for the duration of
        // the call, and the slice does not outlive it (`f` borrows it). A
        // `CArgv` exists only as the parameter Ruby filled.
        unsafe { core::slice::from_raw_parts(argv.0, usize::try_from(argc).unwrap_or(0)) }
    };
    // SAFETY: the caller's contract, which is `finish`'s.
    unsafe { finish(|ruby| f(ruby, args)) }
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
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, S, $($t,)* R> $method<S, $($t,)* R> for F
        where
            F: Fn(S $(, $t)*) -> R,
            S: TryConvert,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)(S::try_convert(ruby, s)? $(, $t::try_convert(ruby, $a)?)*).into_return(ruby)
            }
        }

        #[doc(hidden)]
        pub trait $ruby_method<S, $($t,)* R> {
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, S, $($t,)* R> $ruby_method<S, $($t,)* R> for F
        where
            F: for<'r> Fn(&'r Ruby, S $(, $t)*) -> R,
            S: TryConvert,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)(ruby, S::try_convert(ruby, s)? $(, $t::try_convert(ruby, $a)?)*).into_return(ruby)
            }
        }

        #[doc(hidden)]
        pub trait $function<$($t,)* R> {
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, $($t,)* R> $function<$($t,)* R> for F
        where
            F: Fn($($t),*) -> R,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, ruby: &Ruby, _: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)($($t::try_convert(ruby, $a)?),*).into_return(ruby)
            }
        }

        #[doc(hidden)]
        pub trait $ruby_function<$($t,)* R> {
            fn invoke(self, ruby: &Ruby, s: Value $(, $a: Value)*) -> Result<Value, Error>;
        }

        impl<F, $($t,)* R> $ruby_function<$($t,)* R> for F
        where
            F: for<'r> Fn(&'r Ruby $(, $t)*) -> R,
            $($t: TryConvert,)*
            R: ReturnValue,
        {
            #[inline]
            fn invoke(self, ruby: &Ruby, _: Value $(, $a: Value)*) -> Result<Value, Error> {
                (self)(ruby $(, $t::try_convert(ruby, $a)?)*).into_return(ruby)
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
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, S, R> MethodCAry<S, R> for F
where
    F: for<'a> Fn(S, &'a [Value]) -> R,
    S: TryConvert,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(S::try_convert(ruby, s)?, args).into_return(ruby)
    }
}

/// Arity -1, with `&Ruby` and a receiver: `f(&Ruby, S, &[Value])`.
#[doc(hidden)]
pub trait RubyMethodCAry<S, R> {
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, S, R> RubyMethodCAry<S, R> for F
where
    F: for<'r, 'a> Fn(&'r Ruby, S, &'a [Value]) -> R,
    S: TryConvert,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(ruby, S::try_convert(ruby, s)?, args).into_return(ruby)
    }
}

/// Arity -1, no receiver: `f(&[Value])`.
#[doc(hidden)]
pub trait FunctionCAry<R> {
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, R> FunctionCAry<R> for F
where
    F: for<'a> Fn(&'a [Value]) -> R,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, ruby: &Ruby, _: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(args).into_return(ruby)
    }
}

/// Arity -1, with `&Ruby` and no receiver: `f(&Ruby, &[Value])`.
#[doc(hidden)]
pub trait RubyFunctionCAry<R> {
    fn invoke(self, ruby: &Ruby, s: Value, args: &[Value]) -> Result<Value, Error>;
}

impl<F, R> RubyFunctionCAry<R> for F
where
    F: for<'r, 'a> Fn(&'r Ruby, &'a [Value]) -> R,
    R: ReturnValue,
{
    #[inline]
    fn invoke(self, ruby: &Ruby, _: Value, args: &[Value]) -> Result<Value, Error> {
        (self)(ruby, args).into_return(ruby)
    }
}

/// `method!(f, n)`: `f` as a Ruby method of arity `n` (-1 for `argc`/`argv`),
/// for `define_method`.
#[macro_export]
macro_rules! method {
    ($f:expr, -1) => { $crate::__trampoline!(cary $f, MethodCAry, RubyMethodCAry) };
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
    ($f:expr, -1) => { $crate::__trampoline!(cary $f, FunctionCAry, RubyFunctionCAry) };
    ($f:expr, 0) => { $crate::__trampoline!(function $f, new0, Function0, RubyFunction0;) };
    ($f:expr, 1) => { $crate::__trampoline!(function $f, new1, Function1, RubyFunction1; a0) };
    ($f:expr, 2) => { $crate::__trampoline!(function $f, new2, Function2, RubyFunction2; a0, a1) };
    ($f:expr, 3) => { $crate::__trampoline!(function $f, new3, Function3, RubyFunction3; a0, a1, a2) };
    ($f:expr, 4) => { $crate::__trampoline!(function $f, new4, Function4, RubyFunction4; a0, a1, a2, a3) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trampoline {
    (cary $f:expr, $plain:ident, $ruby:ident) => {{
        extern "C" fn anon(
            argc: ::core::ffi::c_int,
            argv: $crate::rb::method::CArgv,
            rb_self: $crate::rb::Value,
        ) -> $crate::rb::Value {
            #[allow(unused_imports)]
            use $crate::rb::method::{$plain as _, $ruby as _};
            /* Bound OUTSIDE the `unsafe` block: inside it, an unsafe
             * operation written in `$f` would need no `unsafe` of its own,
             * and the caller's `forbid(unsafe_code)` sees none in an
             * external macro's expansion. */
            let f = $f;
            // SAFETY: Ruby is calling this trampoline, with the arguments it
            // passed.
            unsafe {
                $crate::rb::method::finish_cary(argc, argv, |ruby, args| f.invoke(ruby, rb_self, args))
            }
        }
        $crate::rb::method::Func::cary(anon)
    }};
    ($kind:ident $f:expr, $new:ident, $plain:ident, $ruby:ident; $($a:ident),*) => {{
        extern "C" fn anon(
            rb_self: $crate::rb::Value
            $(, $a: $crate::rb::Value)*
        ) -> $crate::rb::Value {
            #[allow(unused_imports)]
            use $crate::rb::method::{$plain as _, $ruby as _};
            /* Bound outside the `unsafe` block, as the `cary` arm's. */
            let f = $f;
            // SAFETY: Ruby is calling this trampoline.
            unsafe { $crate::rb::method::finish(|ruby| f.invoke(ruby, rb_self $(, $a)*)) }
        }
        $crate::rb::method::Func::$new(anon)
    }};
}

/// `rb_block_given_p`: whether the method running now was given a block.
/// Raises nothing and runs no Ruby code; like [`keyword_given`], it reads
/// the current frame.
#[inline]
pub fn block_given(_: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { rb_sys::rb_block_given_p() != 0 }
}

/// Yield `v` to the method's block (`rb_yield`): the caller's Ruby, which
/// can raise, `break`, `throw` or `return`. Each comes back as `Err` and is
/// passed on with `?`, so the trampoline resumes it as it left. Each call is
/// an `allow` of `runs_callers_ruby` at its site, saying why the block runs
/// there.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "the one way a method yields to its block; each caller's own allow says why"
    )
)]
pub fn yield_value(ruby: &Ruby, v: Value) -> Result<Value, Error> {
    crate::raw::yield_value(ruby, v)
}

/// [`yield_value`], and what the block's exit or exception came back as
/// DROPPED: a test suite's stand-in for a method that loses an `Err`, to
/// check that the trampoline still resumes the exit. Here, in the layer,
/// since an extension that forbids dropping an error cannot write it.
#[doc(hidden)]
pub fn yield_dropping_error_for_test(ruby: &Ruby) {
    crate::raw::yield_dropping_error_for_test(ruby)
}

/// `rb_keyword_given_p`: whether the method running now was called with
/// keywords - the one thing `rb_scan_args` asks Ruby when it separates them.
/// Raises nothing and runs no Ruby code. It reads the current frame, so it
/// answers for a method only from that method's own body, before the method
/// calls into Ruby (a method's argument scan is called first thing).
#[inline]
pub fn keyword_given(_: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { rb_sys::rb_keyword_given_p() != 0 }
}

/// `rb_ext_ractor_safe(true)`: every method the running `Init_` defines from
/// here on may be called from any Ractor, where Ruby would refuse it outside
/// the main one (`Ractor::UnsafeError`). Ruby keeps the declaration for the
/// `require` that runs this `Init_` alone. Raises nothing and runs no Ruby.
///
/// The layer's part holds already: what it keeps for the process is
/// [`Shareable`](super::Shareable), a non-local exit is pending per thread,
/// and no wrapped object can leave its Ractor (it is never shareable, and,
/// its class having no allocator, cannot be copied or moved to another). The
/// extension's part is its own: its statics, what its methods assume the GVL
/// serialises (only one Ractor's threads now), and no Marshal hook on a
/// class of wrapped objects, through which a Ractor would copy one.
pub fn declare_ractor_safe(_: &Ruby) {
    // SAFETY: under the GVL (`&Ruby`); sets the running thread's extension
    // config.
    unsafe { rb_sys::rb_ext_ractor_safe(true) }
}

/// What the extension's `Init_` runs its two phases through.
///
/// `check` runs first and defines nothing - a check that the binary suits
/// the running Ruby, say - so its `Err` is raised as it is: a `LoadError`
/// a caller may rescue, with nothing of the extension left behind. Once
/// `init` has begun to define the extension, anything that stops it - an
/// `Err` or a panic - is Ruby's `fatal`: an extension that stopped half-way
/// through defining itself must not be used (its classes lack methods, and
/// the internal error may not be registered yet), and a second `require`
/// would only run `Init_` again over the half it left.
///
/// # Safety
/// Called only from the extension's `Init_` function, which Ruby calls on its
/// thread with the GVL held: as [`finish`], this makes the token.
#[doc(hidden)]
pub unsafe fn run_init(
    check: fn(&Ruby) -> Result<(), Error>,
    init: fn(&Ruby) -> Result<(), Error>,
) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::Init, _>(|| {
        // SAFETY: Ruby calls an extension's `Init_` on its thread, with the GVL.
        let ruby = unsafe { Ruby::get_unchecked() };
        let msg = match std::panic::catch_unwind(AssertUnwindSafe(|| check(&ruby))) {
            Ok(Ok(())) => match std::panic::catch_unwind(AssertUnwindSafe(|| {
                /* What the layer asks Ruby once, before any of the extension
                 * runs - and after `check`, which is the first thing to
                 * touch the interpreter. */
                crate::encoding::init(&ruby);
                init(&ruby)
            })) {
                Ok(Ok(())) => return,
                /* `quietly`: formatting is what the compiler cannot show
                 * unwind-free (`rake no_unwind`). */
                Ok(Err(e)) => crate::caught::quietly(
                    crate::msg::Msg::copy("the extension stopped while defining itself"),
                    || {
                        crate::msg::Msg::format(format_args!(
                            "the extension stopped while defining itself: {}",
                            e.wording(&ruby)
                        ))
                    },
                ),
                Err(payload) => crate::caught::panic_message(payload),
            },
            Ok(Err(e)) => e.raise(&ruby),
            Err(payload) => crate::caught::panic_message(payload),
        };
        match crate::raw::exception_new(&ruby, ruby.exception_runtime_error(), msg.as_bytes()) {
            Ok(exc) => crate::raw::exc_fatal(exc),
            Err(e) => e.raise(&ruby),
        }
    })
}
