//! The raw machinery behind [`TypedType`](super::TypedType): reading an
//! object's data pointer, and making an object with its data. The
//! `rb_data_type_t` it carries is `nupur_sys`'s ([`DataType`]).

#![allow(unsafe_code)]

use core::ffi::c_void;

pub(super) use nupur_sys::data_type::DataType;

use super::Envelope;
use crate::rb::value::AsSys;
use crate::rb::{Error, Value};

/// The data pointer of a TypedData object of type `ty` (or a type deriving
/// from it), or the `TypeError` Ruby's own check raises.
///
/// The type is tested first, so a well-typed object - every call on the normal
/// path - makes no protected call: its data pointer is read directly. Only a
/// mismatch runs `rb_check_typeddata` (protected), which is what keeps the
/// error message Ruby's own, word for word, on every supported Ruby.
///
/// The pointer is never null: an object whose data was never allocated is a
/// `TypeError` too. `rb_data_typed_object_zalloc` makes a non-embedded type's
/// object before its data, so an allocation that failed leaves an object of
/// the class with none, which `ObjectSpace` can still hand to Ruby code until
/// it is collected; every reader of a payload comes through here or
/// [`typed_data_known`].
pub(super) fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let sv = v.as_sys();
    // SAFETY: `v` is a live value and `ty` a registered data type; the data
    // pointer is read only once the type is known to match.
    unsafe {
        if nupur_sys::non_raising::rb_typeddata_is_kind_of(sv, ty) {
            let p = data_pointer(sv);
            if p.is_null() {
                return Err(crate::rb::error::type_error(
                    "an object whose data was never allocated",
                ));
            }
            return Ok(p);
        }
        match crate::raw::check_typeddata(sv, ty) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(crate::rb::error::type_error("wrong argument type")),
        }
    }
}

/// The data pointer of a TypedData object, read without a check.
///
/// # Safety
/// `v` is a TypedData object.
#[inline]
pub(super) unsafe fn data_pointer(v: nupur_sys::value::Value) -> *mut c_void {
    // SAFETY: the caller's contract.
    unsafe { nupur_sys::non_raising::rtypeddata_get_data(v) }
}

/// The data pointer of a TypedData object whose type the caller has already
/// established - a receiver its `TryConvert` checked, or the owner a
/// checked node holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// the method's trampoline makes it the internal error (`rb::error::INTERNAL_ERROR`), where a raise
/// would longjmp past them.
pub(super) fn typed_data_known(v: Value, ty: &'static DataType) -> *mut c_void {
    let sv = v.as_sys();
    // SAFETY: as in `typed_data`; the assert is the type check.
    unsafe {
        assert!(
            nupur_sys::non_raising::rb_typeddata_is_kind_of(sv, ty),
            "a VALUE of an established type had a different one"
        );
        let p = data_pointer(sv);
        /* An established object was checked (`typed_data`) or made whole
         * (`wrap_built`), so its data is there. */
        assert!(!p.is_null(), "an established object without its data");
        p
    }
}

/// Make a `klass` object of data type `ty` with room for an envelope, and
/// write `data` and `roots` into it: `TypedData_Make_Struct`, the C API's own
/// way to make an object with its data, for every type - embedded or not,
/// which only decides where Ruby puts the data (`rtypeddata_get_data` finds it
/// either way).
///
/// One C call, `rb_data_typed_object_zalloc`, makes the object and its data,
/// so one protected call per object. A non-embedded type's data is its
/// `xcalloc`, which also makes it a point where Ruby decides whether malloc
/// pressure calls for a collection: `rb_gc_adjust_memory_usage`, which a
/// payload reports external memory with, only adds to that pressure, so a loop of
/// wraps with no Ruby allocation of their data ran no GC at all
/// (`spec/gc_accounting_spec.rb`).
///
/// The order is the point. The data is zeroed until the envelope is written,
/// which is not a valid payload, so the payload is built BEFORE the object
/// exists, and nothing runs between the allocation and the write: no mark,
/// free or size callback can see the zeroed data, and a panic building the
/// payload leaves no object at all. The roots are on this frame through the
/// allocation (the caller's array), where the GC's stack scan keeps them
/// alive and unmoved, and are in the slots - where the mark finds them -
/// before anything else runs. A failed allocation - `Err`, from `raw`'s
/// protected call - drops the payload.
///
/// # Safety
/// Under the GVL; `ty` must be the type of `T` with `N` slots; each root is
/// a live value.
pub(super) unsafe fn wrap_built<T, const N: usize>(
    klass: nupur_sys::value::Value,
    ty: &'static DataType,
    data: T,
    roots: [nupur_sys::value::Root; N],
) -> Result<nupur_sys::value::Value, Error> {
    const { assert!(core::mem::size_of::<Envelope<T, N>>() != 0) };
    const { assert!(core::mem::align_of::<Envelope<T, N>>() <= 16) }; /* malloc's alignment */
    let obj = crate::raw::typed_object_zalloc(klass, core::mem::size_of::<Envelope<T, N>>(), ty)?;
    let p = data_pointer(obj) as *mut Envelope<T, N>;
    if p.is_null() {
        return Err(crate::rb::error::internal_error(
            "a TypedData object was made without its data",
        ));
    }
    /* The roots are read here, after the allocation - the one point a
     * collection can run - so they stayed on this frame across it; from
     * here the mark sees them in the slots. */
    p.write(Envelope { roots, data });
    Ok(obj)
}
