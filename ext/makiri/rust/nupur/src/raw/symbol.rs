//! Symbols and ids.

use super::*;

/// `rb_intern3` of a UTF-8 name: raises `NoMemoryError` when the symbol table
/// cannot grow (interning valid UTF-8 raises nothing else).
#[inline]
pub fn intern(_: &Ruby, name: &str) -> Result<Id, Error> {
    let (p, n) = (name.as_ptr() as *const c_char, c_len(name.len())?);
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern3(p, n, rb_sys::rb_utf8_encoding())
    })
    .map(Id::from_raw)
}

/// `rb_intern` of a NUL-terminated name: as [`intern`].
#[inline]
pub fn intern_cstr(_: &Ruby, name: &core::ffi::CStr) -> Result<Id, Error> {
    let p = name.as_ptr();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern(p)
    })
    .map(Id::from_raw)
}

/// The Symbol of the UTF-8 `name`: [`intern`] and `rb_id2sym`, which raises
/// `NoMemoryError` when a dynamic Symbol is made.
#[inline]
pub(crate) fn sym(ruby: &Ruby, name: &str) -> Result<Symbol, Error> {
    let id = intern(ruby, name)?.as_raw();
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_id2sym(id)
    })?;
    typed(v, Symbol::from_value, "a C call made no Symbol")
}
