//! The TypedData objects, and the GC callbacks Ruby drives.
//!
//! A wrapped value is a Rust struct behind a `rb_data_type_t`. Ruby calls the
//! three function pointers in that struct directly, so they have the C calling
//! convention and receive a raw data pointer - the raw Ruby ABI, and therefore
//! this layer's job (see `glue/mod.rs`). The struct declares what to do through
//! [`Hooks`], and the callbacks below are the one monomorphised bridge from
//! Ruby's GC to it, so no glue module writes an `extern "C"` GC function.
//!
//! The object outlives the wrapper struct with `ruby_xfree` as its allocator
//! (`bridge::ruby::wrap_zeroed`), so the free callback releases what the struct
//! owns and then frees it, in one place.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void};

use rb_sys::{rb_data_type_t, VALUE};

use super::ruby::DataType;

/// The mark phase's handle, for [`Hooks::mark`].
pub struct Marker(());

impl Marker {
    /// Mark a stored `VALUE` as reachable.
    #[inline]
    pub fn mark(&self, v: VALUE) {
        // SAFETY: called from Ruby's mark phase, with the GVL held.
        unsafe { rb_sys::rb_gc_mark(v) };
    }
}

/// A Rust value owned by a Ruby object.
///
/// Exactly one method is required: what Ruby values the object keeps alive.
/// `memsize` sizes the object for the GC and defaults to its Rust size;
/// `release` frees what the object owns and defaults to nothing (the wrapper
/// struct itself is always freed by the bridge).
pub trait Hooks: Sized {
    /// Mark every `VALUE` this object holds.
    fn mark(&self, marker: &Marker);

    /// The bytes this object owns, reported to Ruby's GC.
    fn memsize(&self) -> usize {
        core::mem::size_of::<Self>()
    }

    /// Release what the object owns, before the bridge frees the struct.
    fn release(&mut self) {}
}

/* The three GC callbacks below are the crate's only `extern "C"` functions
 * with no panic latch, and deliberately so. They run from Ruby's collector,
 * where there is no frame to raise into: `mark_cb` cannot report a failure
 * without risking premature collection of what it did not mark, and `free_cb`
 * cannot report one without leaking or freeing twice. So a panic here keeps
 * the old behaviour - Rust turns the unwind at the boundary into an abort -
 * which is the honest answer when the alternative is a corrupted heap.
 * Keep their bodies trivial; that is what makes the choice cheap. */

unsafe extern "C" fn mark_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: Ruby hands back the pointer `wrap_zeroed` stored, a live `T`.
    unsafe { (*(ptr as *mut T)).mark(&Marker(())) };
}

unsafe extern "C" fn free_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: as `mark_cb`; the object is being collected, so this is its last
    // access, and the struct came from `ruby_xcalloc`, so it goes back to
    // `ruby_xfree`.
    unsafe {
        (*(ptr as *mut T)).release();
        rb_sys::ruby_xfree(ptr);
    }
}

unsafe extern "C" fn memsize_cb<T: Hooks>(ptr: *const c_void) -> rb_sys::size_t {
    // SAFETY: as `mark_cb`.
    unsafe { (*(ptr as *const T)).memsize() as rb_sys::size_t }
}

/// A `rb_data_type_t` whose GC callbacks drive [`Hooks`] for `T`.
///
/// `const`, so a module can keep it in a `static` as the C would have; a
/// derived type passes its base's `as_ptr()` as `parent`.
pub const fn data_type<T: Hooks>(name: *const c_char, parent: *const rb_data_type_t) -> DataType {
    DataType::new(
        name,
        parent,
        Some(mark_cb::<T>),
        Some(free_cb::<T>),
        Some(memsize_cb::<T>),
    )
}

/// `rb_typeddata_is_kind_of`, for the kind discriminators that choose a leaf
/// class by representation rather than by Ruby class.
#[inline]
pub fn kind_of(v: VALUE, ty: &DataType) -> bool {
    // SAFETY: `v` is a live VALUE; the type check does not raise.
    unsafe { rb_sys::rb_typeddata_is_kind_of(v, ty.as_ptr()) != 0 }
}
