//! The GC callbacks Ruby drives for every [`TypedType`](super::TypedType):
//! the one monomorphised bridge from Ruby's collector to [`Payload`].

#![allow(unsafe_code)]

use core::ffi::c_void;

use super::{Envelope, Payload};

/* The GC callbacks below are the crate's only `extern "C"` functions with no
 * panic latch, and deliberately so. They run from Ruby's collector, where
 * there is no frame to raise into: `mark_cb` cannot report a failure without
 * risking premature collection of what it did not mark, and `free_cb` cannot
 * report one without leaking or freeing twice. So a panic here keeps the old
 * behaviour - Rust turns the unwind at the boundary into an abort - which is
 * the honest answer when the alternative is a corrupted heap. Keep their
 * bodies trivial; that is what makes the choice cheap.
 *
 * Each passes over a null data pointer. `rb_data_typed_object_zalloc` makes
 * the object before it allocates a non-embedded type's data, so a failed
 * allocation leaves an object with none; CRuby's collector does not call a
 * callback for it today, but that is its implementation, not its documented
 * contract, and one branch here means nothing rests on it. */

pub(super) unsafe extern "C" fn mark_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMark, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: Ruby hands back the data pointer of an object `wrap_built`
        // made, which holds a live envelope (see `wrap_built`). Only the slots
        // are read: no reference to the payload is made, which a method may be
        // reading when the collector runs.
        let roots = unsafe { core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).roots).read() };
        for r in roots {
            // SAFETY: this is a dmark callback, in Ruby's mark phase, with the
            // GVL held; a slot holds a live value (or `Qfalse`); and this
            // frame loses nothing to a longjmp (the no_unwind guard's Drop
            // does nothing unless unwinding).
            unsafe { nupur_sys::read::gc_mark_in_dmark(r) };
        }
    })
}

/// Give back what the payload reported, once it is dropped.
#[inline]
fn give_back(ruby: &nupur_sys::value::Ruby, reported: usize) {
    if reported != 0 {
        let n = nupur_sys::ssize_t::try_from(reported).unwrap_or(nupur_sys::ssize_t::MAX);
        // SAFETY: a plain counter adjustment, from the free hook, where it
        // only subtracts - as Ruby's own `xfree` does from here; the GC runs
        // its callbacks with the GVL held.
        unsafe { nupur_sys::read::rb_gc_adjust_memory_usage(ruby, n.saturating_neg()) };
    }
}

pub(super) unsafe extern "C" fn free_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFree, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `mark_cb`; the object is being collected, so this is its
        // last access - the payload is dropped exactly once, here, and never
        // read again - and the data came from `rb_data_typed_object_zalloc`'s
        // `xcalloc`, so it goes back to `ruby_xfree`, as
        // `TypedData_Make_Struct`'s data does.
        /* The drop is a read and `quietly`: a type whose drop the compiler
         * cannot show unwind-free (a tree's recursive glue) is caught rather
         * than unwound into the collector. */
        // SAFETY: the GC runs its callbacks on a Ruby thread holding the
        // GVL: the token, made once here and handed down.
        let ruby = unsafe { nupur_sys::value::Ruby::get_unchecked() };
        unsafe {
            let data = core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).data).read();
            let reported = crate::caught::quietly(0, || data.reported_external());
            crate::caught::quietly((), || drop(data));
            give_back(&ruby, reported);
            nupur_sys::read::ruby_xfree(&ruby, ptr);
        }
    })
}

pub(super) unsafe extern "C" fn free_embedded_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFreeEmbedded, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `free_cb`, for an embeddable type: the payload is dropped
        // and the memory is left to Ruby - the object's own slot when it was
        // embedded, and data Ruby frees after this callback when it was not.
        let data = unsafe { core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).data).read() };
        // SAFETY: as `free_cb`: the GVL is held, and the token is made once.
        let ruby = unsafe { nupur_sys::value::Ruby::get_unchecked() };
        let reported = crate::caught::quietly(0, || data.reported_external());
        crate::caught::quietly((), || drop(data));
        give_back(&ruby, reported);
    })
}

/// The bytes the object owns beyond what Ruby counts itself. For a plain
/// type that is the whole envelope (Ruby counts its slot, not the data it
/// points at); for an embeddable one Ruby counts the envelope - as part of
/// the slot it is embedded in, or by the allocation's own size when it is
/// not - so only what the payload owns beyond its struct is left.
pub(super) unsafe extern "C" fn memsize_cb<T: Payload, const N: usize, const EMBEDDABLE: bool>(
    ptr: *const c_void,
) -> nupur_sys::size_t {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMemsize, _>(|| {
        if ptr.is_null() {
            return 0;
        }
        /* `quietly`: the size is advisory, so a panic in it - an overflow
         * check in a sum, say - answers 0 rather than unwinding into the
         * collector. (Not so for mark: a value left unmarked is freed while
         * still referenced, so it must be unwind-free itself.) */
        // SAFETY: as `mark_cb`.
        let data = unsafe { &(*(ptr as *const Envelope<T, N>)).data };
        let slots = core::mem::size_of::<[nupur_sys::value::Root; N]>();
        let bytes = crate::caught::quietly(0, || {
            if EMBEDDABLE {
                data.memsize().saturating_sub(core::mem::size_of::<T>())
            } else {
                data.memsize().saturating_add(slots)
            }
        });
        nupur_sys::size_t::try_from(bytes).unwrap_or(nupur_sys::size_t::MAX)
    })
}
