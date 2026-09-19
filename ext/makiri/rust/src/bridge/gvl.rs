//! Releasing the GVL around Ruby-free work.
//!
//! `rb_thread_call_without_gvl` is a raw Ruby C call, and the body it runs is
//! forbidden from touching Ruby state - not a `Value`, not a `Ruby` handle, not
//! anything Ruby owns. There is no way to check that, so it is the caller's
//! contract, stated on [`without_gvl`].

#![allow(unsafe_code)]

use core::ffi::c_void;

/// Run `f` with the GVL released, and return its result.
///
/// # Contract
/// `f` must touch no Ruby state and allocate no Ruby object. Its argument is
/// typically a slice copied out of Ruby before the call, as `glue::doc`'s
/// parse does.
///
/// A panic inside `f` is CAUGHT and re-raised here, after the GVL is back. It
/// has to be: `f` runs below `rb_thread_call_without_gvl`, a C frame, and
/// unwinding into one aborts the process. This is the whole parser, so it is
/// the single largest piece of Rust that runs under a callback - see
/// [`crate::caught`].
pub fn without_gvl<F: FnOnce() -> R, R>(f: F) -> R {
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
