//! Calling a block and walking a Hash.

use super::*;

/// `rb_yield`: runs the caller's block - Ruby code, which can raise, `break`,
/// `throw` or `return`. Every one of those comes back as `Err` (the
/// non-exception exits as the jump `rb_protect` caught), and is raised again
/// at the method's return, so it reaches the block's caller as it left.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "rb_yield: the wrapper `rb::method::yield_value` is, and its callers allow it"
    )
)]
pub fn yield_value(_: &Ruby, v: Value) -> Result<Value, Error> {
    let v = v.as_raw();
    let out = call(|inside| unsafe { nupur_sys::raising::rb_yield(inside, v) })?;
    // SAFETY: what the block returned, live on this frame.
    Ok(unsafe { Value::from_raw(out) })
}

/// [`yield_value`], and what the block's exit or exception came back as
/// DROPPED (`rb::method::yield_dropping_error_for_test`).
#[doc(hidden)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "the test hook's block, which is the point of it"
    )
)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        discarded_ruby_error,
        reason = "dropping the block's exit is what the test hook is for"
    )
)]
pub fn yield_dropping_error_for_test(ruby: &Ruby) {
    let _ = yield_value(ruby, crate::rb::value::nil(ruby));
}

/// `rb_hash_foreach` over `h`, `visit` answering after each pair whether to
/// go on. Raises `RuntimeError` when the Hash is changed in a way the walk
/// cannot survive (a key added or the table rehashed).
///
/// `visit` is Rust called from C, inside `rb_hash_foreach`'s frames, so a
/// panic in it must not unwind into them: it is latched here - the walk
/// stops - and resumed once the C frames are gone. It never raises either:
/// it answers `false` and the caller keeps its own error.
pub fn hash_foreach(h: RHash, visit: &mut dyn FnMut(Value, Value) -> bool) -> Result<(), Error> {
    struct Walk<'a> {
        visit: &'a mut dyn FnMut(Value, Value) -> bool,
        latch: crate::caught::PanicLatch,
    }
    unsafe extern "C" fn each(k: VALUE, v: VALUE, arg: VALUE) -> c_int {
        const CONTINUE: c_int = nupur_sys::st_retval::ST_CONTINUE as c_int;
        const STOP: c_int = nupur_sys::st_retval::ST_STOP as c_int;
        crate::no_unwind::must_not_unwind::<crate::no_unwind::HashEach, _>(|| {
            // SAFETY: `arg` is the `Walk` `hash_foreach` passed, live for the
            // walk; `k` and `v` are the Hash's, live while it is walked.
            let walk = unsafe { &mut *(arg as *mut Walk<'_>) };
            let (k, v) = unsafe { (Value::from_raw(k), Value::from_raw(v)) };
            let visit = &mut walk.visit;
            if walk.latch.guard(false, || visit(k, v)) {
                CONTINUE
            } else {
                STOP
            }
        })
    }
    let mut walk = Walk {
        visit,
        latch: crate::caught::PanicLatch::new(),
    };
    let (h, arg) = (h.as_raw(), &mut walk as *mut Walk<'_> as VALUE);
    let walked = call(|inside| unsafe {
        nupur_sys::raising::rb_hash_foreach(inside, h, Some(each), arg);
        nupur_sys::Qnil as VALUE
    });
    /* Out of the C frames: safe to unwind from here. */
    walk.latch.resume();
    walked.map(|_| ())
}
