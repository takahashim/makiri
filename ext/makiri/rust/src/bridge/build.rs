//! Ruby objects the glue builds - Arrays, Hashes, Integers, Floats, Symbols -
//! each made through `raw`'s protected calls, so a failed allocation is `Err`.
//!
//! magnus's own constructors (`Ruby::ary_new`, `Ruby::hash_new`,
//! `Ruby::integer_from_i64`, ...) raise instead, a `longjmp` over the caller's
//! frames; `clippy.toml` bans them outside this module and init, so the glue
//! spells every one of these here. Strings are `bridge::string`'s.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_long};

use magnus::{Error, RArray, RHash, Value};

use super::ruby::value;

/// An empty Array with room for `capa` elements.
pub fn ary(capa: usize) -> Result<RArray, Error> {
    let capa = c_long::try_from(capa).unwrap_or(c_long::MAX);
    // SAFETY: no argument to keep live.
    let v = unsafe { value(super::raw::ary_new_capa(capa)?) };
    RArray::from_value(v).ok_or_else(|| super::ruby::internal_error("the allocator made no Array"))
}

/// Append `v` to `a`.
pub fn push(a: RArray, v: Value) -> Result<(), Error> {
    use magnus::rb_sys::AsRawValue;
    // SAFETY: both are live values the caller holds.
    unsafe { super::raw::ary_push(a.as_raw(), v.as_raw()) }.map(|_| ())
}

/// An empty Hash.
pub fn hash() -> Result<RHash, Error> {
    // SAFETY: no argument to keep live.
    let v = unsafe { value(super::raw::hash_new()?) };
    RHash::from_value(v).ok_or_else(|| super::ruby::internal_error("the allocator made no Hash"))
}

/// `h[k] = v`, for a String or Symbol `k` (see `raw::hash_aset`).
pub fn aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    use magnus::rb_sys::AsRawValue;
    // SAFETY: all three are live values the caller holds.
    unsafe { super::raw::hash_aset(h.as_raw(), k.as_raw(), v.as_raw()) }.map(|_| ())
}

/// An Integer.
pub fn int(n: i64) -> Result<Value, Error> {
    // SAFETY: no argument to keep live.
    unsafe { super::raw::ll2inum(n) }.map(|v| unsafe { value(v) })
}

/// An Integer from an unsigned value.
pub fn uint(n: u64) -> Result<Value, Error> {
    // SAFETY: no argument to keep live.
    unsafe { super::raw::ull2inum(n) }.map(|v| unsafe { value(v) })
}

/// A Float.
pub fn float(f: f64) -> Result<Value, Error> {
    // SAFETY: no argument to keep live.
    unsafe { super::raw::float_new(f) }.map(|v| unsafe { value(v) })
}

/// `obj.enum_for(meth)`: an Enumerator over `obj`'s method `meth`.
pub fn enumerator(obj: Value, meth: &str) -> Result<Value, Error> {
    use magnus::rb_sys::AsRawValue;
    let meth = sym(meth)?;
    // SAFETY: both are live values the caller holds.
    unsafe { super::raw::enumeratorize(obj.as_raw(), meth.as_raw()) }.map(|v| unsafe { value(v) })
}

/// The Symbol named by the UTF-8 `name`.
pub fn sym(name: &str) -> Result<Value, Error> {
    // SAFETY: `name` is `name.len()` UTF-8 bytes, read for the call; the
    // encoding is Ruby's static UTF-8 one.
    let id = unsafe {
        super::raw::intern3(
            name.as_ptr() as *const c_char,
            name.len() as c_long,
            rb_sys::rb_utf8_encoding(),
        )
    }?;
    // SAFETY: an interned ID.
    unsafe { super::raw::id2sym(id) }.map(|v| unsafe { value(v) })
}
