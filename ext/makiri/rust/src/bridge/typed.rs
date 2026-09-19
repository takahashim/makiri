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
//! ([`wrap_zeroed`]), so the free callback releases what the struct
//! owns and then frees it, in one place.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;

use magnus::rb_sys::AsRawValue;
use magnus::{Error, Value};
use rb_sys::{rb_data_type_t, VALUE};

use super::ruby::protect;

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

/// A `rb_data_type_t` for `T`: its GC callbacks drive [`Hooks`] for `T`, and
/// every way to make or read an object of this type goes through it.
///
/// The type parameter is the point. The raw API took a `*const rb_data_type_t`
/// beside a separate `T`, so wrapping a `NodeData` under a Document's type -
/// and reading it back as the wrong struct - compiled. Here the data type and
/// the struct are one value, so the pairing cannot be wrong.
///
/// `const`, so a module keeps it in a `static`, as the C did.
pub struct TypedType<T: Hooks> {
    raw: DataType,
    _t: PhantomData<fn() -> T>,
}

impl<T: Hooks> TypedType<T> {
    const fn with_parent(name: *const c_char, parent: *const rb_data_type_t) -> TypedType<T> {
        TypedType {
            raw: DataType::new(
                name,
                parent,
                Some(mark_cb::<T>),
                Some(free_cb::<T>),
                Some(memsize_cb::<T>),
            ),
            _t: PhantomData,
        }
    }

    /// A base type.
    pub const fn base(name: *const c_char) -> TypedType<T> {
        Self::with_parent(name, core::ptr::null())
    }

    /// A type deriving from `parent`, which holds the same `T`: an object of
    /// this type is also one of `parent`'s.
    pub const fn derived(name: *const c_char, parent: &TypedType<T>) -> TypedType<T> {
        Self::with_parent(name, parent.raw.as_ptr())
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&self, v: Value) -> bool {
        // SAFETY: `v` is a live VALUE; the type check does not raise.
        unsafe { rb_sys::rb_typeddata_is_kind_of(v.as_raw(), self.raw.as_ptr()) != 0 }
    }

    /// The `T` behind `v`, or Ruby's own `TypeError`.
    ///
    /// Borrowed for as long as the caller borrows `v`, the object that owns the
    /// data - so the reference cannot outlive the VALUE it came from.
    pub fn get<'v>(&'static self, v: &'v Value) -> Result<&'v T, Error> {
        let p = typed_data(*v, &self.raw)? as *const T;
        // SAFETY: `typed_data` verified the type, and the wrapper owns the data.
        Ok(unsafe { &*p })
    }

    /// [`get`](Self::get) for a VALUE whose type the caller already
    /// established; a mismatch is a bug in that reasoning, and panics.
    pub fn get_known<'v>(&'static self, v: &'v Value) -> &'v T {
        // SAFETY: as `get`; `known_ptr` asserts the type.
        unsafe { &*self.known_ptr(*v) }
    }

    /// The `T` behind an object whose type the caller established, for a
    /// write. The pointer is valid while `v` is rooted.
    pub fn known_ptr(&'static self, v: Value) -> *mut T {
        typed_data_known(v, &self.raw) as *mut T
    }

    /// Allocate a zeroed `T`, fill it with `init`, wrap it as a `klass` object
    /// of this type, and only then let `store` write the VALUEs it holds - see
    /// [`wrap_zeroed`] for why that order.
    ///
    /// # Safety
    /// Under the GVL; `T` must be valid when zeroed.
    pub unsafe fn wrap(
        &'static self,
        klass: VALUE,
        init: impl FnOnce(&mut T),
        store: impl FnOnce(&mut T),
    ) -> VALUE {
        wrap_zeroed::<T>(klass, self.raw.as_ptr(), init, store)
    }
}

/* ---- the raw machinery behind TypedType ---- */

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
struct DataType(rb_sys::rb_data_type_t);

// SAFETY: the contents are set once at compile time and never mutated. Ruby
// reads them from whichever thread holds the GVL.
unsafe impl Sync for DataType {}

impl DataType {
    /// `parent` is null for a base type.
    pub const fn new(
        name: *const core::ffi::c_char,
        parent: *const rb_sys::rb_data_type_t,
        dmark: rb_sys::RUBY_DATA_FUNC,
        dfree: rb_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> rb_sys::size_t>,
    ) -> DataType {
        DataType(rb_sys::rb_data_type_t {
            wrap_struct_name: name,
            function: rb_sys::rb_data_type_struct__bindgen_ty_1 {
                dmark,
                dfree,
                dsize,
                dcompact: None,
                reserved: [core::ptr::null_mut(); 1],
            },
            parent,
            data: core::ptr::null_mut(),
            flags: rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE,
        })
    }

    /// The raw pointer the Ruby API wants.
    #[inline]
    pub const fn as_ptr(&self) -> *const rb_sys::rb_data_type_t {
        self as *const DataType as *const rb_sys::rb_data_type_t
    }
}

/// The data pointer of a TypedData object of type `ty` (or a type deriving
/// from it), or the `TypeError` Ruby's own check raises.
///
/// The type is tested first, so a well-typed object - every call on the normal
/// path - never enters `protect`. Only a mismatch runs `rb_check_typeddata`
/// under it, which is what keeps the error message Ruby's own, word for word,
/// on every supported Ruby.
fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: `v` is a live value and `ty` a registered data type. The check
    // runs unprotected only once the type is known to match, so it cannot
    // raise there; a mismatch runs it under `protect`.
    unsafe {
        if rb_sys::rb_typeddata_is_kind_of(v, ty) != 0 {
            return Ok(rb_sys::rb_check_typeddata(v, ty));
        }
        match protect(|| rb_sys::rb_check_typeddata(v, ty) as VALUE) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(Error::new(
                magnus::Ruby::get_unchecked().exception_type_error(),
                "wrong argument type",
            )),
        }
    }
}

/// The data pointer of a TypedData object whose type the caller has already
/// established - a receiver magnus converted, or the Document a checked node
/// holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// magnus turns it into a fatal error, where a raise would longjmp past them.
fn typed_data_known(v: Value, ty: &'static DataType) -> *mut c_void {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: as in `typed_data`; the assert makes the check that follows one
    // that cannot raise.
    unsafe {
        assert!(
            rb_sys::rb_typeddata_is_kind_of(v, ty) != 0,
            "a VALUE of an established type had a different one"
        );
        rb_sys::rb_check_typeddata(v, ty)
    }
}

/// Allocate a zeroed `T`, fill it with `init`, wrap it as a `klass` object of
/// data type `ty`, and only then let `store` write the VALUEs it holds.
///
/// The order is the point. The wrap allocates, so it is a GC point, and a VALUE
/// already sitting in this malloc'd struct is seen by no mark there: a GC can
/// free it, or compaction move it out from under the stored copy. Zeroed, a
/// VALUE field reads as `false` to the mark until `store` sets it; and the
/// VALUEs `store` writes are still on the caller's stack across the wrap,
/// where the conservative scan pins them.
///
/// `ruby_xcalloc` raises `NoMemoryError` on OOM; nothing is owned at that
/// point, which is the fallible-allocation line for glue-side buffers.
///
/// # Safety
/// Under the GVL. `T` must be valid when zeroed, and `ty` must free it with
/// `ruby_xfree`.
unsafe fn wrap_zeroed<T>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    init: impl FnOnce(&mut T),
    store: impl FnOnce(&mut T),
) -> VALUE {
    let data = rb_sys::ruby_xcalloc(1, core::mem::size_of::<T>() as rb_sys::size_t) as *mut T;
    init(&mut *data);
    let obj = rb_sys::rb_data_typed_object_wrap(klass, data as *mut c_void, ty);
    store(&mut *data);
    obj
}

/* ---- magnus's TypedData, for NodeSet ----
 * NodeSet is a magnus `#[derive(TypedData)]` (its GC hooks are magnus's), not
 * a `TypedType`; this is its one unchecked accessor, kept beside the others. */

/// The wrapped Rust value behind a TypedData object, without magnus's
/// `rb_protect`.
///
/// `<&T>::try_convert` - and so every magnus method with a wrapped receiver -
/// runs `rb_check_typeddata` inside `rb_protect`, which is a `setjmp` per call.
/// That is the right default when a Rust caller wants a `Result`, but it is not
/// free: on the per-node path it measured about a quarter of the throughput of
/// the C it replaced (`Node#css` over 2000 nodes, `notes/node_set_ab.rb`).
///
/// # Safety
/// Raises (longjmps) when `v` is not a `T`, so no Rust destructor may be live.
/// Only for a VALUE the caller built as a `T` itself. The returned lifetime is
/// unconstrained; the caller must keep `v` rooted.
pub(in crate::bridge) unsafe fn typed_data_unprotected<'a, T: magnus::TypedData>(
    v: VALUE,
) -> &'a T {
    /* magnus::DataType is #[repr(transparent)] over rb_data_type_t, so this
     * cast is what the repr promises; the accessor for it is crate-private. */
    let dt = T::data_type() as *const magnus::typed_data::DataType as *const rb_data_type_t;
    &*(rb_sys::rb_check_typeddata(v, dt) as *const T)
}
