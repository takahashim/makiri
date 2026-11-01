//! Strings and their encodings.

use super::*;

/// `rb_utf8_str_new` of `bytes`: raises `NoMemoryError`. The String is tagged
/// UTF-8 without a check: `bytes` that were not would make a wrong String,
/// never a memory error.
#[inline]
pub(crate) fn utf8_str_new(_: &Ruby, bytes: &[u8]) -> Result<RString, Error> {
    let (p, n) = (bytes.as_ptr() as *const c_char, c_len(bytes.len())?);
    let v = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_utf8_str_new(p, n)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// A UTF-8 String of `len` bytes whose content is NOT written:
/// `rb_utf8_str_new(NULL, len)`, which raises `NoMemoryError`. What
/// `string::utf8_str_filled` writes in place.
///
/// # Safety
/// Before Ruby code can see the String, every one of its `len` bytes is
/// written or its length set to 0: its buffer holds whatever the allocation
/// held.
#[inline]
pub(crate) unsafe fn utf8_str_unwritten(_: &Ruby, len: usize) -> Result<RString, Error> {
    let n = c_len(len)?;
    let v = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_utf8_str_new(core::ptr::null(), n)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// `rb_enc_interned_str` of `bytes` in `enc`: raises `NoMemoryError`.
#[inline]
pub fn enc_interned_str(bytes: &[u8], enc: Encoding) -> Result<RString, Error> {
    let (p, n, e) = (
        bytes.as_ptr() as *const c_char,
        c_len(bytes.len())?,
        enc.as_raw(),
    );
    let v = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_enc_interned_str(p, n, e)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// `rb_str_dup`: raises `NoMemoryError`.
#[inline]
pub fn str_dup(s: RString) -> Result<RString, Error> {
    let v = s.as_raw();
    let d = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_str_dup(v)
    })?;
    typed(d, RString::from_value, "a C call made no String")
}

/// `rb_enc_associate`: `s` itself, tagged `enc`. Raises `FrozenError` for a
/// frozen String, and `ArgumentError` for an encoding its bytes cannot carry.
#[inline]
pub fn enc_associate(s: RString, enc: Encoding) -> Result<RString, Error> {
    let (v, e) = (s.as_raw(), enc.as_raw());
    let r = call(|| unsafe { nupur_sys::rb_enc_associate(v, e) })?;
    typed(r, RString::from_value, "a C call made no String")
}

/// `rb_str_encode` to `enc` with `flags`: raises `Encoding::ConverterNotFoundError`
/// when Ruby has no converter, and - without replacement flags -
/// `Encoding::InvalidByteSequenceError` / `UndefinedConversionError`; the new
/// String's allocation raises `NoMemoryError`.
#[inline]
pub fn str_encode(s: RString, enc: Encoding, flags: c_int) -> Result<RString, Error> {
    let (v, e) = (s.as_raw(), enc.as_raw());
    let r = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_str_encode(
            v,
            nupur_sys::rb_enc_from_encoding(e),
            flags,
            nupur_sys::Qnil as VALUE,
        )
    })?;
    typed(r, RString::from_value, "a C call made no String")
}

/// `rb_to_encoding` of an `Encoding` or a String: raises `ArgumentError` for
/// an unknown name. Any other value is a `TypeError` here, before the call,
/// which would convert it with its `to_str`.
#[inline]
pub fn to_encoding(ruby: &Ruby, v: Value) -> Result<Encoding, Error> {
    use crate::rb::ReprValue;
    if !v.is_kind_of(ruby.class_encoding()) && RString::from_value(v).is_none() {
        return Err(crate::rb::error::type_error(
            "an Encoding or a String was expected",
        ));
    }
    let r = v.as_raw();
    let e = call(|| unsafe { nupur_sys::rb_to_encoding(r) })?;
    // SAFETY: Ruby's encodings live for the process.
    Ok(unsafe { Encoding::from_raw(e) })
}

/// `rb_enc_find_index` + `rb_enc_from_index`: may autoload the encoding,
/// which can raise (`LoadError`) and is a GC point; `None` for a name Ruby
/// does not know. Not `rb_enc_find`, which answers ASCII-8BIT for one.
#[inline]
pub fn enc_find(_: &Ruby, name: &core::ffi::CStr) -> Result<Option<Encoding>, Error> {
    let p = name.as_ptr();
    let e = call(|| unsafe {
        let index = nupur_sys::rb_enc_find_index(p);
        if index < 0 {
            core::ptr::null_mut()
        } else {
            nupur_sys::rb_enc_from_index(index)
        }
    })?;
    // SAFETY: a non-null answer is one of Ruby's encodings, which live for
    // the process.
    Ok((!e.is_null()).then(|| unsafe { Encoding::from_raw(e) }))
}
