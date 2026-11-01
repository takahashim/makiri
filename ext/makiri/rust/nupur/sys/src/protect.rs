//! How a raise becomes an error value, and how one is raised again: the
//! protection Nupur runs every C call that can raise under ([`call`]), what
//! a protected call that came back with a raise or a non-local exit leaves
//! behind - the thread's error info (`$!`), and an exit pending until the
//! trampoline resumes it - and the calls that leave on purpose
//! ([`exc_raise`], [`resume`], [`exc_fatal`]).
//!
//! Nothing here knows Nupur's types: a raise comes back as the raw
//! exception ([`Caught`]), which Nupur makes its `Error`. Every function that
//! calls Ruby's C is an `unsafe fn` whose contract is the GVL, held on a Ruby
//! thread; the one exception is the thunk, which Ruby itself calls.

#![allow(unsafe_code)]

use core::ffi::c_int;
use core::marker::PhantomData;

use crate::VALUE;

/// The proof that code runs inside [`call`]'s `rb_protect`, where a raise
/// comes back to the caller instead of `longjmp`ing over its frames.
///
/// Only the thunk makes one, and lends it to the protected closure for the
/// call alone: the closure takes `&Protected` for any lifetime, and its result
/// is a `T` chosen outside, so the reference cannot leave the call. It is
/// neither `Send` nor `Sync`, so a GVL-released body cannot carry it either.
///
/// Not made outside this crate, so a wrapper of [`crate::raising`] cannot be
/// called outside the protection (each rustdoc checks the error code, so a
/// build that failed for another reason does not pass):
///
/// ```compile_fail,E0451
/// let made = nupur_sys::protect::Protected { _not_send: core::marker::PhantomData };
/// let _ = unsafe { nupur_sys::raising::rb_hash_new(&made) };
/// ```
///
/// kept out of a place that outlives the call:
///
/// ```compile_fail,E0521
/// let kept = core::cell::Cell::new(None);
/// let at = &kept;
/// let _ = unsafe { nupur_sys::protect::call(move |p| at.set(Some(p))) };
/// ```
///
/// and from another thread:
///
/// ```compile_fail,E0277
/// fn sent<T: Send>(_: T) {}
/// let _ = unsafe { nupur_sys::protect::call(|p| sent(p)) };
/// ```
pub struct Protected {
    _not_send: PhantomData<*mut ()>,
}

/// What a protected call came back with instead of its value.
#[derive(Clone, Copy)]
pub enum Caught {
    /// A raise: the exception Ruby left in `$!`, taken (`$!` is clear again).
    /// A live value, which the caller keeps on its stack.
    Exception(VALUE),
    /// Any other non-local exit (a `break` or `throw` out of a block, a thread
    /// kill), by its `rb_protect` state - now pending ([`pending_exit`]).
    Jump(c_int),
    /// The closure did not run, though `rb_protect` reported no exit: an
    /// internal error, never an answer.
    NotRun,
}

/// The raise sweep's hook (`script/check_raise_injection.rb`): in an
/// `alloc-inject` build the armed allocating call raises `NoMemoryError` in
/// place of its allocation, as a real failure would - so the sweep can make
/// every one of them raise in turn. Nothing in any other build. It takes the
/// token, so the raise is protected like the call's. Whether a call is armed
/// is the extension's to answer ([`set_raise_injector`]).
#[inline(always)]
pub fn injected_raise(_: &Protected) {
    #[cfg(feature = "alloc-inject")]
    if RAISE_INJECTOR.get().is_some_and(|armed| armed()) {
        /* Raises without allocating: Ruby keeps the exception preallocated.
         * SAFETY: inside the protection, which the token proves. */
        unsafe { rb_sys::rb_memerror() }
    }
}

/// The function that answers whether the next allocating call raises.
#[cfg(feature = "alloc-inject")]
static RAISE_INJECTOR: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Register the raise sweep's answer, once (Nupur's `inject::install`, which
/// registers the allocation half with it); a second registration is ignored.
#[cfg(feature = "alloc-inject")]
pub fn set_raise_injector(armed: fn() -> bool) {
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
    /// and the GVL is not released (`nupur::gvl::without_gvl` answers the exit
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
pub fn reset_pending_exit() {
    PENDING_JUMP.set(0);
}

/// The `rb_protect` state of the exit pending, or `None`.
#[inline]
pub fn pending_exit() -> Option<c_int> {
    match PENDING_JUMP.get() {
        0 => None,
        state => Some(state),
    }
}

/// Resume the non-local exit `state` stands for, which a protected call
/// caught: the one way a pending exit leaves the layer.
///
/// # Safety
///
/// Under the GVL, as a trampoline's last act, with nothing owned on the
/// frames it leaves: `rb_jump_tag` `longjmp`s.
#[inline]
pub unsafe fn resume(state: c_int) -> ! {
    reset_pending_exit();
    // SAFETY: `state` is one `rb_protect` reported; resuming it is the point,
    // and the caller owns nothing it skips.
    unsafe { crate::rb_jump_tag(state) }
}

/// Run `f` - one C call, and the `Copy` values it reads - under a bare
/// `rb_protect`; a raise comes back as [`Caught::Exception`], and `$!` is
/// cleared (an error the caller handles rather than raises is run under
/// [`keeping_errinfo`]). Once an exit is pending, `f` does not run: the call
/// answers that exit.
///
/// `F: Copy` and `T: Copy` are what keep the thunk's frame free of anything a
/// skipped destructor would leak: a closure is `Copy` only when everything it
/// captures is, so it owns nothing, and neither does its result. There is no
/// `catch_unwind` here (a raise crossing one is undefined behaviour); a panic
/// in the thunk would abort at its `extern "C"` boundary, and the closures
/// Nupur passes hold no Rust that panics. The one C call that calls back into
/// Rust, `rb_hash_foreach`, latches its callback's panic inside the callback,
/// and the callback raises nothing.
///
/// # Safety
///
/// On a Ruby thread, with the GVL held. `f` may call Ruby's C, which may
/// raise; it must own nothing a `longjmp` would skip.
#[inline]
pub unsafe fn call<T: Copy, F: FnOnce(&Protected) -> T + Copy>(f: F) -> Result<T, Caught> {
    struct Slot<F, T> {
        f: F,
        out: Option<T>,
    }
    unsafe extern "C" fn thunk<T: Copy, F: FnOnce(&Protected) -> T + Copy>(arg: VALUE) -> VALUE {
        crate::no_unwind::must_not_unwind::<crate::no_unwind::ProtectThunk, _>(|| {
            // SAFETY: `arg` is the `Slot` `call` passed, live for the call.
            let slot = unsafe { &mut *(arg as *mut Slot<F, T>) };
            let inside = Protected {
                _not_send: PhantomData,
            };
            slot.out = Some((slot.f)(&inside));
            crate::Qnil as VALUE
        })
    }
    if let Some(state) = pending_exit() {
        return Err(Caught::Jump(state));
    }
    let mut slot = Slot { f, out: None };
    let mut state: c_int = 0;
    // SAFETY: the thunk reads only `slot`, which outlives the call; the GVL
    // is the caller's.
    unsafe {
        crate::rb_protect(
            Some(thunk::<T, F>),
            &mut slot as *mut Slot<F, T> as VALUE,
            &mut state,
        );
    }
    match (state, slot.out) {
        (0, Some(out)) => Ok(out),
        (0, None) => Err(Caught::NotRun),
        // SAFETY: a protected call just returned, under the caller's GVL.
        (state, _) => Err(unsafe { caught(state) }),
    }
}

/// Run `f`, protected calls whose error the caller handles rather than
/// raises - a lookup that may fail, a name that may not be made - with `$!`
/// put back as it was after, as Ruby's own C does around one
/// (`load_encoding` saves `rb_errinfo()` and restores it): a caught raise
/// clears it, and a handled one must not clear the caller's. Not every call
/// does this: the read costs a call into libruby, and an error that is raised
/// replaces `$!` anyway.
///
/// Nothing is put back once an exit is pending: Ruby resumes it from the
/// error info the exit left there, which `before` would overwrite.
///
/// # Safety
///
/// On a Ruby thread, with the GVL held.
#[inline]
pub unsafe fn keeping_errinfo<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: under the GVL, as the contract says; a read.
    let before = unsafe { crate::rb_errinfo() };
    let out = f();
    if pending_exit().is_none() {
        // SAFETY: as above; `before` was `$!` a moment ago, kept on this
        // frame's stack since.
        unsafe { restore_errinfo(before) };
    }
    out
}

/// Put `$!` back to `before`. Only nil or an exception goes back, which is
/// all `rb_set_errinfo` takes without raising.
///
/// # Safety
///
/// Under the GVL; `before` a live value.
#[inline]
unsafe fn restore_errinfo(before: VALUE) {
    if before == crate::Qnil as VALUE {
        // SAFETY: nil, which it always takes.
        unsafe { crate::rb_set_errinfo(before) };
        return;
    }
    // SAFETY: `before` is live (the contract); `rb_eException` a C global
    // Ruby set at boot. `rb_obj_is_kind_of` raises only for a non-class
    // second argument.
    if unsafe { crate::rb_obj_is_kind_of(before, crate::rb_eException) } == crate::Qtrue as VALUE {
        // SAFETY: an exception, which `rb_set_errinfo` takes without raising.
        unsafe { crate::rb_set_errinfo(before) };
    }
}

/// The `rb_protect` state of a raise (`TAG_RAISE` in CRuby's `vm_core.h`).
/// Not part of the documented API - the state is called opaque - but it is
/// how a raise is told from a `throw` or a `break`, whose state must be
/// resumed untouched; magnus reads the same value. Unchanged since 1.9.
const TAG_RAISE: c_int = 6;

/// `$!`, taken: the exception a raise left there, with `$!` set back to
/// `nil` (`rb_errinfo`, `rb_set_errinfo`). Raises nothing.
///
/// # Safety
///
/// Under the GVL.
#[inline]
unsafe fn take_errinfo() -> VALUE {
    // SAFETY: under the GVL (the contract); both read or write the current
    // thread's error info.
    unsafe {
        let exc = crate::rb_errinfo();
        crate::rb_set_errinfo(crate::Qnil as VALUE);
        exc
    }
}

/// Clear the error info a protected call left behind: [`call`] takes it only
/// for a raise it recognises, so an exit it did not (the load-time self-check
/// under a Ruby that numbers its states otherwise) leaves the exception there,
/// to become the `cause` of whatever is raised next.
///
/// # Safety
///
/// Under the GVL.
#[inline]
pub unsafe fn forget_errinfo() {
    // SAFETY: the contract.
    let _ = unsafe { take_errinfo() };
}

/// `rb_exc_raise`: raise `exc`. Raises, by design.
///
/// # Safety
///
/// Under the GVL, with nothing owned on the frames it leaves; `exc` a live
/// exception object.
#[inline]
pub unsafe fn exc_raise(exc: VALUE) -> ! {
    // SAFETY: the contract.
    unsafe { crate::rb_exc_raise(exc) }
}

/// `rb_exc_fatal`: raise `exc` as Ruby's `fatal`. Raises, by design.
///
/// # Safety
///
/// As [`exc_raise`].
#[inline]
pub unsafe fn exc_fatal(exc: VALUE) -> ! {
    // SAFETY: the contract.
    unsafe { crate::rb_exc_fatal(exc) }
}

/// What a non-zero `rb_protect` state stands for: the exception Ruby left in
/// `$!` for a raise (taken, so `$!` is clear again), or the state itself for
/// any other exit - which is then pending ([`PENDING_JUMP`]).
///
/// # Safety
///
/// Under the GVL, right after the protected call returned.
#[inline]
unsafe fn caught(state: c_int) -> Caught {
    if state == TAG_RAISE {
        // SAFETY: the contract.
        let exc = unsafe { take_errinfo() };
        if exc != crate::Qnil as VALUE {
            return Caught::Exception(exc);
        }
    }
    PENDING_JUMP.set(state);
    Caught::Jump(state)
}
