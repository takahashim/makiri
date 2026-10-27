//! Typed data and Ruby's allocator.

use super::*;

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub(crate) unsafe fn check_typeddata(
    v: VALUE,
    ty: *const rb_data_type_t,
) -> Result<*mut c_void, Error> {
    call(|| unsafe { rb_sys::rb_check_typeddata(v, ty) })
}

/// `rb_data_typed_object_zalloc`: raises `NoMemoryError`. For an embeddable
/// type small enough, the zeroed data is the object's own slot.
#[inline]
pub(crate) unsafe fn typed_object_zalloc(
    klass: VALUE,
    size: usize,
    ty: *const rb_data_type_t,
) -> Result<VALUE, Error> {
    let size = rb_sys::size_t::try_from(size)
        .map_err(|_| crate::rb::error::internal_error("a wrapped object too large"))?;
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_data_typed_object_zalloc(klass, size, ty)
    })
}
