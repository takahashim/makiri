//! Typed data and Ruby's allocator.

use super::*;

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub(crate) unsafe fn check_typeddata(
    v: nupur_sys::value::Value,
    ty: *const rb_data_type_t,
) -> Result<*mut c_void, Error> {
    call(|inside| unsafe { nupur_sys::raising::rb_check_typeddata(inside, v, ty) })
}

/// `rb_data_typed_object_zalloc`: raises `NoMemoryError`. For an embeddable
/// type small enough, the zeroed data is the object's own slot.
#[inline]
pub(crate) unsafe fn typed_object_zalloc(
    klass: nupur_sys::value::Value,
    size: usize,
    ty: *const rb_data_type_t,
) -> Result<nupur_sys::value::Value, Error> {
    let size = nupur_sys::size_t::try_from(size)
        .map_err(|_| crate::rb::error::internal_error("a wrapped object too large"))?;
    call(|inside| unsafe {
        injected_raise(inside);
        nupur_sys::raising::rb_data_typed_object_zalloc(inside, klass, size, ty)
    })
}
