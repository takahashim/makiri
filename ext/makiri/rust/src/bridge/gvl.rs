//! Releasing the GVL around Ruby-free work.
//!
//! `rb_thread_call_without_gvl` is a raw Ruby C call, and the body it runs is
//! forbidden from touching Ruby state - not a `Value`, not a `Ruby` handle, not
//! anything Ruby owns. There is no way to check that, so it is the caller's
//! contract, stated on [`without_gvl`].
//!
//! The other direction - proving the GVL IS held - is [`held`], which turns a
//! `Ruby` handle into the [`Gvl`] token the process-global CSS engines ask for.
//! `without_gvl` requires a `Send` body so no token can be carried across.

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::gvl::Gvl;

/// The GVL token for a thread that has a `Ruby` handle.
///
/// magnus hands out a `Ruby` only on a Ruby thread holding the GVL, and caches
/// that answer per thread - so a handle obtained inside [`without_gvl`] would
/// wrongly succeed. That is the body `without_gvl`'s contract already forbids
/// touching Ruby, and its `Send` bound keeps both a `&Ruby` and a `Gvl` out of
/// the closure's captures.
pub fn held(_ruby: &magnus::Ruby) -> Gvl {
    // SAFETY: a `Ruby` handle is magnus's proof that this thread holds the GVL,
    // and the token cannot outlive the frame that has it across a release
    // (see above).
    unsafe { Gvl::assume() }
}

/// Run `f` with the GVL released, and return its result - or the exception a
/// pending interrupt raised instead.
///
/// # Contract
/// `f` must touch no Ruby state and allocate no Ruby object. It must also be
/// `Send`, which is what keeps a `Ruby` handle and a [`Gvl`] - neither is - out
/// of it. Its argument is
/// typically a slice copied out of Ruby before the call, as `bridge::doc`'s
/// parse does.
///
/// A panic inside `f` is CAUGHT and re-raised here, after the GVL is back. It
/// has to be: `f` runs below the GVL-releasing call, a C frame, and unwinding
/// into one aborts the process. This is the whole parser, so it is the single
/// largest piece of Rust that runs under a callback - see [`crate::caught`].
///
/// # Interrupts
/// `rb_thread_call_without_gvl` checks for pending interrupts before and after
/// the body and RAISES there - a longjmp over this frame and its callers, which
/// skips their destructors: the parsed document in `out` and the copied source
/// leaked on every `Timeout`/`Thread#raise` that landed during a parse (30
/// interrupted 5 MB parses grew the process by ~900 MB). The `2` variant never
/// raises: an interrupt pending at the start makes it return without running
/// the body, and it does not check afterwards. So:
///
/// - The body ran: its result is returned. An interrupt that arrived meanwhile
///   is delivered by Ruby's next check, after the result is a Ruby object that
///   the GC owns - nothing leaks.
/// - It did not run: the interrupt is delivered HERE, under `protect`, so it
///   comes back as `Err` through ordinary Rust returns. If delivering it raised
///   nothing (a trap handler that returned), the call is simply made again.
pub fn without_gvl<F: FnOnce() -> R + Send, R>(f: F) -> Result<R, magnus::Error> {
    struct Slot<F, R> {
        f: Option<F>,
        out: Option<R>,
        panic: crate::caught::PanicLatch,
    }

    unsafe extern "C" fn run<F: FnOnce() -> R, R>(p: *mut c_void) -> *mut c_void {
        // SAFETY: `p` is the `&mut Slot` passed below, live for this call.
        let slot = unsafe { &mut *(p as *mut Slot<F, R>) };
        /* Never a panic here: this frame is Ruby's C, outside the guard below,
         * so one would abort the process. The body is present on the first
         * run; if it somehow were not, `out` stays None and the caller reports
         * that under the GVL. */
        let Some(f) = slot.f.take() else {
            return core::ptr::null_mut();
        };
        /* Catch here rather than let the panic reach Ruby's C frame: `out`
         * simply stays None, and the caller re-raises once the GVL is back. */
        let out = &mut slot.out;
        slot.panic.guard((), || *out = Some(f()));
        core::ptr::null_mut()
    }

    let mut slot = Slot {
        f: Some(f),
        out: None,
        panic: crate::caught::PanicLatch::new(),
    };
    loop {
        // SAFETY: the trampoline runs `f` at most once and returns; the unblock
        // function is Ruby's default (`None`), as in the C call this replaces.
        // This variant raises nothing (see above), so no Rust frame is jumped.
        unsafe {
            rb_sys::rb_thread_call_without_gvl2(
                Some(run::<F, R>),
                &mut slot as *mut Slot<F, R> as *mut c_void,
                None,
                core::ptr::null_mut(),
            );
        }
        /* Back under the GVL and out of the C frame: safe to unwind from here. */
        slot.panic.resume();
        if let Some(out) = slot.out.take() {
            return Ok(out);
        }
        if slot.f.is_none() {
            /* It ran and neither answered nor panicked - which `run` cannot do. */
            return Err(magnus::Error::new(
                crate::init::EXC_INTERNAL_ERROR.exception(),
                "the GVL-released body ran without a result",
            ));
        }
        /* An interrupt was pending, so the body never started: deliver it. */
        // SAFETY: under the GVL; `protect` turns the raise into `Err`.
        crate::bridge::ruby::protect(|| {
            unsafe { rb_sys::rb_thread_check_ints() };
            rb_sys::Qnil as rb_sys::VALUE
        })?;
    }
}
