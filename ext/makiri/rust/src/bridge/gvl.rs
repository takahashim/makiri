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

/// Run `f` with the GVL released, and return its result.
///
/// # Contract
/// `f` must touch no Ruby state and allocate no Ruby object. It must also be
/// `Send`, which is what keeps a `Ruby` handle and a [`Gvl`] - neither is - out
/// of it. Its argument is
/// typically a slice copied out of Ruby before the call, as `glue::doc`'s
/// parse does.
///
/// A panic inside `f` is CAUGHT and re-raised here, after the GVL is back. It
/// has to be: `f` runs below `rb_thread_call_without_gvl`, a C frame, and
/// unwinding into one aborts the process. This is the whole parser, so it is
/// the single largest piece of Rust that runs under a callback - see
/// [`crate::caught`].
pub fn without_gvl<F: FnOnce() -> R + Send, R>(f: F) -> R {
    struct Slot<F, R> {
        f: Option<F>,
        out: Option<R>,
        panic: crate::caught::PanicLatch,
    }

    unsafe extern "C" fn run<F: FnOnce() -> R, R>(p: *mut c_void) -> *mut c_void {
        // SAFETY: `p` is the `&mut Slot` passed below, live for this call.
        let slot = unsafe { &mut *(p as *mut Slot<F, R>) };
        let f = slot.f.take().expect("the GVL-released body runs once");
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
    // SAFETY: the trampoline runs `f` once and returns; the unblock function is
    // Ruby's default (`None`), as in the C call this replaces.
    unsafe {
        rb_sys::rb_thread_call_without_gvl(
            Some(run::<F, R>),
            &mut slot as *mut Slot<F, R> as *mut c_void,
            None,
            core::ptr::null_mut(),
        );
    }
    /* Back under the GVL and out of the C frame: safe to unwind from here. */
    slot.panic.resume();
    slot.out.expect("the GVL-released body ran")
}
