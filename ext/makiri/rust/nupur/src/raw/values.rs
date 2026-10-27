//! Arrays, Hashes and numbers (`build` spells these for a caller).

use super::*;

/// `rb_ary_new_capa`: raises `NoMemoryError`.
#[inline]
pub(crate) fn ary_new_capa(_: &Ruby, capa: usize) -> Result<RArray, Error> {
    let n = crate::int::capacity(capa);
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_new_capa(n)
    })?;
    typed(v, RArray::from_value, "a C call made no Array")
}

/// `rb_ary_push`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// Array.
#[inline]
pub(crate) fn ary_push(a: RArray, v: Value) -> Result<(), Error> {
    let (a, v) = (a.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_push(a, v)
    })
    .map(|_| ())
}

/// `rb_ivar_set` for a hidden ivar (a name without `@`, so Ruby code cannot
/// see it): raises `FrozenError` for a frozen object and
/// `NoMemoryError`.
#[inline]
pub fn ivar_set(obj: Value, id: Id, v: Value) -> Result<(), Error> {
    let (o, v, id) = (obj.as_raw(), v.as_raw(), id.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ivar_set(o, id, v)
    })
    .map(|_| ())
}

/// `rb_hash_lookup2` at the Fixnum `n`: a Fixnum hashes and compares by
/// value in C, so no Ruby code runs; protected all the same, and a raise
/// passed on, as [`hash_lookup`]'s is.
#[inline]
pub(crate) fn table_get(h: RHash, n: u32) -> Result<Option<Value>, Error> {
    const UNDEF: VALUE = rb_sys::Qundef as VALUE;
    /* No entry has a key past a Fixnum - past 2^30 - 1 on Windows. */
    let Some(key) = crate::int::fixnum(i64::from(n)) else {
        return Ok(None);
    };
    let h = h.as_raw();
    let v = call(|| unsafe { rb_sys::rb_hash_lookup2(h, key, UNDEF) })?;
    // SAFETY: a value of the Hash's, or `Qundef`, which is not handed out.
    Ok((v != UNDEF).then(|| unsafe { Value::from_raw(v) }))
}

/// `rb_hash_aset` at the Fixnum `n`: raises `NoMemoryError`.
#[inline]
pub(crate) fn table_set(h: RHash, n: u32, v: Value) -> Result<(), Error> {
    /* No entry for a key past a Fixnum: refused, never wrapped round. */
    let key = crate::int::fixnum(i64::from(n)).ok_or_else(|| {
        Error::new(
            crate::rb::Builtin::RangeError,
            "a table key past a Fixnum (more than 2^30 nodes on Windows)",
        )
    })?;
    let (h, v) = (h.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, key, v)
    })
    .map(|_| ())
}

/// A key whose hash Ruby computes itself ([`crate::rb::value::plain_key`]),
/// or the `TypeError` for one whose `#hash` and `#eql?` would be called.
#[inline]
pub(super) fn plain_key_or_refuse(k: Value) -> Result<(), Error> {
    if crate::rb::value::plain_key(k) {
        return Ok(());
    }
    Err(crate::rb::error::type_error(
        "a Hash key must be a Symbol or a String (not a subclass's)",
    ))
}

/// `rb_hash_new`: raises `NoMemoryError`.
#[inline]
pub(crate) fn hash_new(_: &Ruby) -> Result<RHash, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_new()
    })?;
    typed(v, RHash::from_value, "a C call made no Hash")
}

/// A Hash no Ruby code can find: `rb_hash_new`, then `rb_obj_hide` with
/// nothing between, so `ObjectSpace` never lists it - an object's private
/// table ([`crate::table::IntTable`]), which is never shown. Raises
/// `NoMemoryError`. Hand it to no Ruby code: a hidden object has no class.
pub(crate) fn hidden_hash_new(ruby: &Ruby) -> Result<RHash, Error> {
    let h = hash_new(ruby)?;
    // SAFETY: the Hash just made, referenced only from this frame.
    unsafe { crate::rb::value::obj_hide(crate::rb::ReprValue::as_value(h)) };
    Ok(h)
}

/// `rb_hash_aset` for a Symbol or String key, whose hash Ruby computes
/// itself ([`crate::rb::value::plain_key`]): any other key - an instance of a
/// String subclass included - would have its `#hash` and `#eql?` called, so
/// it is a `TypeError` here. Raises `NoMemoryError`, and `FrozenError` for a
/// frozen Hash.
#[inline]
pub(crate) fn hash_aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    plain_key_or_refuse(k)?;
    let (h, k, v) = (h.as_raw(), k.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, k, v)
    })
    .map(|_| ())
}

/// `rb_ll2inum`: raises `NoMemoryError` for a value past a Fixnum (a Bignum).
#[inline]
pub(crate) fn int_from_i64(_: &Ruby, n: i64) -> Result<Integer, Error> {
    /* A Fixnum is the value's own bits: no allocation, nothing to raise, no
     * protected call - the common case (a length, an index, a count). */
    if let Some(v) = crate::int::fixnum(n) {
        // SAFETY: an immediate.
        return Ok(Integer::from_known(unsafe { Value::from_raw(v) }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ll2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_ull2inum`: as [`int_from_i64`].
#[inline]
pub(crate) fn int_from_u64(_: &Ruby, n: u64) -> Result<Integer, Error> {
    /* As `int_from_i64`. */
    if let Some(v) = crate::int::posfixnum(n) {
        // SAFETY: an immediate.
        return Ok(Integer::from_known(unsafe { Value::from_raw(v) }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ull2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_float_new`: raises `NoMemoryError` for a value that is not a flonum.
#[inline]
pub(crate) fn float_new(_: &Ruby, f: f64) -> Result<Float, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_float_new(f)
    })?;
    typed(v, Float::from_value, "a C call made no Float")
}

/// `rb_enumeratorize_with_size` with no arguments and no size: raises
/// `NoMemoryError`.
#[inline]
pub(crate) fn enumeratorize(obj: Value, meth: Symbol) -> Result<Value, Error> {
    let (o, m) = (obj.as_raw(), meth.as_raw());
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_enumeratorize_with_size(o, m, 0, core::ptr::null(), None)
    })?;
    typed(v, Some, "a C call made no value")
}
