//! Typed data and Ruby's allocator.

use super::*;

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub(crate) unsafe fn check_typeddata(
    v: VALUE,
    ty: *const rb_data_type_t,
) -> Result<*mut c_void, Error> {
    // SAFETY: `v` is live, the caller's contract.
    let v = unsafe { nupur_sys::value::Value::from_raw(v) };
    call(|inside| unsafe { nupur_sys::raising::rb_check_typeddata(inside, v, ty) })
}

/// `rb_data_typed_object_zalloc`: raises `NoMemoryError`. For an embeddable
/// type small enough, the zeroed data is the object's own slot.
#[inline]
pub(crate) unsafe fn typed_object_zalloc(
    klass: VALUE,
    size: usize,
    ty: *const rb_data_type_t,
) -> Result<VALUE, Error> {
    let size = nupur_sys::size_t::try_from(size)
        .map_err(|_| crate::rb::error::internal_error("a wrapped object too large"))?;
    // SAFETY: `klass` is a live class, the caller's contract.
    let klass = unsafe { nupur_sys::value::Value::from_raw(klass) };
    call(|inside| unsafe {
        injected_raise(inside);
        nupur_sys::raising::rb_data_typed_object_zalloc(inside, klass, size, ty)
    })
    .map(nupur_sys::value::Value::as_raw)
}
