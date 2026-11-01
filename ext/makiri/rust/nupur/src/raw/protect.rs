//! How a raise becomes an `Err`, and how one is raised again: the
//! protection every wrapper of [`super`] runs its C call under ([`call`]),
//! what a protected call that came back with a raise or a non-local exit
//! leaves behind - the thread's error info (`$!`), and an exit pending until
//! the trampoline resumes it - and the calls that leave on purpose
//! ([`exc_raise`], [`resume`], [`exc_fatal`]).
//!
//! The rest of `raw` is the wrappers, every one a C call made inside
//! [`call`]; `rake dylint` holds it to that (`unraising_call_in_raw`), and
//! this module is the one part of `raw` that calls Ruby's C outside it - the
//! protection itself, `$!`, and the raises - so it is kept to exactly that.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::rb::value::AsRawValue;
use crate::rb::{Error, Ruby, Value};
use nupur_sys::VALUE;

/// The raise sweep's hook (`script/check_raise_injection.rb`): in an
/// `alloc-inject` build the armed allocating call below raises `NoMemoryError`
/// in place of its allocation, as a real failure would - so the sweep can make
/// every one of them raise in turn. Nothing in any other build. Called inside
/// the thunk, so the raise is protected like the call's. Whether a call is
/// armed is the extension's to answer ([`set_raise_injector`]).
#[inline(always)]
pub(super) unsafe fn injected_raise() {
    #[cfg(feature = "alloc-inject")]
    if RAISE_INJECTOR.get().is_some_and(|armed| armed()) {
        /* Raises without allocating: Ruby keeps the exception preallocated. */
        unsafe { nupur_sys::rb_memerror() }
    }
}

/// The function that answers whether the next allocating call raises.
#[cfg(feature = "alloc-inject")]
static RAISE_INJECTOR: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Register the raise sweep's answer, once (`crate::inject::install`, which
/// registers the allocation half with it); a second registration is ignored.
#[cfg(feature = "alloc-inject")]
pub(crate) fn set_raise_injector(armed: fn() -> bool) {
    let _ = RAISE_INJECTOR.set(armed);
}

thread_local! {
    /// A non-local exit that is not an exception (a `break` or `throw` out of a
    /// block, a thread kill) a protected call came back with, not yet resumed by
    /// the trampoline: its `rb_protect` state, 0 for none.
    ///
    /// Ruby resumes such an exit (`rb_jump_tag`) from what it left in the
    /// thread's error info - the throw data, a kill's marker - and every raise
    /// overwrites that. So once one is pending, no protected call runs C at
    /// all, each answering the same exit, [`keeping_errinfo`] puts nothing back,
    /// and the GVL is not released ([`crate::gvl::without_gvl`] answers the exit
    /// too). The trampoline resumes it however the method ends - an `Err` of
    /// another kind, or an `Ok` from a method that dropped the exit's `Err` -
    /// so it is resumed on the thread that caught it, and none is left behind
    /// for the next method.
    ///
    /// The native thread's own, not the process's: two Ractors run their
    /// methods at once, each under its own GVL, and a global would let one
    /// forget the other's exit or resume it on the wrong thread. Nothing between
    /// the exit and its resumption can release the GVL (only a C call can, and
    /// none runs), so the Ruby thread that caught it is still on this native
    /// thread when it is resumed - with M:N threads too, which switch only where
    /// the GVL is released.
    static PENDING_JUMP: core::cell::Cell<c_int> = const { core::cell::Cell::new(0) };
}

/// Forget a pending exit: a new method starts. The trampoline resumes every
/// exit before its method returns, so none is pending here; this is the
/// belt to that brace.
#[inline]
pub(crate) fn reset_pending_exit() {
    PENDING_JUMP.set(0);
}

/// The exit pending, as the `Err` that resumes it, or `None`.
#[inline]
pub(crate) fn pending_exit() -> Option<Error> {
    match PENDING_JUMP.get() {
        0 => None,
        state => Some(Error::jump(state)),
    }
}

/// Resume the non-local exit `state` stands for, which a protected call
/// caught: the one way a pending exit leaves the layer. Only `Error::raise`
/// calls it, as a trampoline's last act, with nothing owned.
#[inline]
pub(crate) fn resume(state: c_int) -> ! {
    reset_pending_exit();
    jump_tag(state)
}

/// Run `f` - one C call, and the `Copy` values it reads - under a bare
/// `rb_protect`; a raise comes back as `Err`, and `$!` is cleared (an error
/// the caller handles rather than raises is run under [`keeping_errinfo`]).
///
/// `F: Copy` and `T: Copy` are what keep the thunk's frame free of anything a
/// skipped destructor would leak: a closure is `Copy` only when everything it
/// captures is, so it owns nothing, and neither does its result. There is no
/// `catch_unwind` here (a raise crossing one is undefined behaviour); a panic
/// in the thunk would abort at its `extern "C"` boundary, and the closures
/// below hold no Rust that panics. The one C call that calls back into Rust,
/// `rb_hash_foreach` ([`hash_foreach`]), latches its callback's panic inside
/// the callback, and the callback raises nothing.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "the raw module's alone: every wrapper that hands it its C call holds the proof"
    )
)]
pub(super) fn call<T: Copy, F: FnOnce() -> T + Copy>(f: F) -> Result<T, Error> {
    struct Slot<F, T> {
        f: F,
        out: Option<T>,
    }
    unsafe extern "C" fn thunk<T: Copy, F: FnOnce() -> T + Copy>(arg: VALUE) -> VALUE {
        crate::no_unwind::must_not_unwind::<crate::no_unwind::ProtectThunk, _>(|| {
            // SAFETY: `arg` is the `Slot` `call` passed, live for the call.
            let slot = unsafe { &mut *(arg as *mut Slot<F, T>) };
            slot.out = Some((slot.f)());
            nupur_sys::Qnil as VALUE
        })
    }
    if let Some(exit) = pending_exit() {
        return Err(exit);
    }
    let mut slot = Slot { f, out: None };
    let mut state: c_int = 0;
    // SAFETY: the thunk reads only `slot`, which outlives the call.
    unsafe {
        nupur_sys::rb_protect(
            Some(thunk::<T, F>),
            &mut slot as *mut Slot<F, T> as VALUE,
            &mut state,
        );
    }
    match (state, slot.out) {
        (0, Some(out)) => Ok(out),
        (0, None) => Err(crate::rb::error::internal_error(
            "a protected call did not run",
        )),
        (state, _) => Err(caught(state)),
    }
}

/// Run `f`, protected calls whose error the caller handles rather than
/// raises - a lookup that may fail, a name that may not be made - with `$!`
/// put back as it was after, as Ruby's own C does around one
/// (`load_encoding` saves `rb_errinfo()` and restores it): a caught raise
/// clears it ([`take_errinfo`]), and a handled one must not clear the
/// caller's. Not every call does this: the read costs a call into libruby,
/// and an error that is raised replaces `$!` anyway.
///
/// Nothing is put back once an exit is pending: Ruby resumes it from the
/// error info the exit left there, which `before` would overwrite.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "crate-private: called only from functions holding a proof, around their protected calls"
    )
)]
pub(crate) fn keeping_errinfo<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: under the GVL, as every caller's proof says; a read.
    let before = unsafe { nupur_sys::rb_errinfo() };
    let out = f();
    if pending_exit().is_none() {
        restore_errinfo(before);
    }
    out
}

/// Put `$!` back to `before`. Only nil or an exception goes back, which is
/// all `rb_set_errinfo` takes without raising.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: `keeping_errinfo`'s, under its caller's proof"
    )
)]
fn restore_errinfo(before: VALUE) {
    if before == nupur_sys::Qnil as VALUE {
        // SAFETY: nil, which it always takes.
        unsafe { nupur_sys::rb_set_errinfo(before) };
        return;
    }
    // SAFETY: `before` was the thread's error info a moment ago, kept on
    // this frame's stack since: a live value; `rb_eException` a C global Ruby
    // set at boot. `rb_obj_is_kind_of` raises only for a non-class second
    // argument.
    if unsafe { nupur_sys::rb_obj_is_kind_of(before, nupur_sys::rb_eException) }
        == nupur_sys::Qtrue as VALUE
    {
        // SAFETY: an exception, which `rb_set_errinfo` takes without raising.
        unsafe { nupur_sys::rb_set_errinfo(before) };
    }
}

/// The `rb_protect` state of a raise (`TAG_RAISE` in CRuby's `vm_core.h`).
/// Not part of the documented API - the state is called opaque - but it is
/// how a raise is told from a `throw` or a `break`, whose state must be
/// resumed untouched; magnus reads the same value. Unchanged since 1.9.
const TAG_RAISE: c_int = 6;

/// `$!`, taken: the exception a raise left there, with `$!` set back to
/// `nil` (`rb_errinfo`, `rb_set_errinfo`). Raises nothing.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: read only right after a protected call returned, on the thread that made it"
    )
)]
fn take_errinfo() -> Value {
    // SAFETY: under the GVL (a protected call just returned); both read or
    // write the current thread's error info.
    unsafe {
        let exc = nupur_sys::rb_errinfo();
        nupur_sys::rb_set_errinfo(nupur_sys::Qnil as VALUE);
        Value::from_raw(exc)
    }
}

/// Clear the error info a protected call left behind: [`caught`] takes it only
/// for a raise it recognises, so an exit it did not (the load-time self-check
/// under a Ruby that numbers its states otherwise) leaves the exception there,
/// to become the `cause` of whatever is raised next.
#[inline]
pub(crate) fn forget_errinfo(_: &Ruby) {
    let _ = take_errinfo();
}

/// `rb_jump_tag`: resume the non-local exit `state` stands for. Raises, by
/// design - only [`resume`] calls it.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: only a trampoline raise resumes an exit, with the GVL"
    )
)]
fn jump_tag(state: c_int) -> ! {
    // SAFETY: `state` is one `rb_protect` reported; resuming it is the point.
    unsafe { nupur_sys::rb_jump_tag(state) }
}

/// `rb_exc_raise`: raise `exc`. Raises, by design - only `Error::raise` calls
/// it, with nothing owned.
#[inline]
pub(crate) fn exc_raise(exc: Value) -> ! {
    // SAFETY: `exc` is a live exception object.
    unsafe { nupur_sys::rb_exc_raise(exc.as_raw()) }
}

/// `rb_exc_fatal`: raise `exc` as Ruby's `fatal` - for a panic in
/// the extension's `Init_`, where a half-defined extension must not be used. Raises,
/// by design.
#[inline]
pub(crate) fn exc_fatal(exc: Value) -> ! {
    // SAFETY: `exc` is a live exception object.
    unsafe { nupur_sys::rb_exc_fatal(exc.as_raw()) }
}

/// The error a non-zero `rb_protect` state stands for: the exception Ruby
/// left in `$!` for a raise (taken, so `$!` is clear again), or the state
/// itself for any other exit - which is then pending ([`PENDING_JUMP`]).
#[inline]
fn caught(state: c_int) -> Error {
    if state == TAG_RAISE {
        let exc = take_errinfo();
        if !crate::rb::ReprValue::is_nil(exc) {
            return Error::exception(exc);
        }
    }
    PENDING_JUMP.set(state);
    Error::jump(state)
}
