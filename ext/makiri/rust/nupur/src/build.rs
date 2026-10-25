//! Ruby objects the glue builds - Arrays, Hashes, Integers, Floats, Symbols -
//! each made through `raw`'s protected calls, so a failed allocation is `Err`.
//!
//! The C constructors raise on a failed allocation, a `longjmp` over the
//! caller's frames, and are `bridge::raw`'s alone; the glue spells every one
//! of these here. Strings are `bridge::string`'s.

#![forbid(unsafe_code)]

use crate::rb::ReprValue;
use crate::rb::{Error, RArray, RHash, Ruby, Value};

use super::raw;

/// An empty Array with room for `capa` elements.
pub fn ary(ruby: &Ruby, capa: usize) -> Result<RArray, Error> {
    raw::ary_new_capa(ruby, capa)
}

/// Append `v` to `a`.
pub fn push(a: RArray, v: Value) -> Result<(), Error> {
    raw::ary_push(a, v)
}

/// An empty Hash.
pub fn hash(ruby: &Ruby) -> Result<RHash, Error> {
    raw::hash_new(ruby)
}

/// `h[k] = v`, for a String or Symbol `k` (any other is a `TypeError`: see
/// `raw::hash_aset`).
pub fn aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    raw::hash_aset(h, k, v)
}

/// An Integer.
pub fn int(ruby: &Ruby, n: i64) -> Result<Value, Error> {
    raw::int_from_i64(ruby, n).map(|i| i.as_value())
}

/// An Integer from an unsigned value.
pub fn uint(ruby: &Ruby, n: u64) -> Result<Value, Error> {
    raw::int_from_u64(ruby, n).map(|i| i.as_value())
}

/// A Float.
pub fn float(ruby: &Ruby, f: f64) -> Result<Value, Error> {
    raw::float_new(ruby, f).map(|f| f.as_value())
}

/// `obj.enum_for(meth)`: an Enumerator over `obj`'s method `meth`.
pub fn enumerator(ruby: &Ruby, obj: Value, meth: &str) -> Result<Value, Error> {
    raw::enumeratorize(obj, raw::sym(ruby, meth)?)
}

/// The Symbol named by the UTF-8 `name`.
pub fn symbol(ruby: &Ruby, name: &str) -> Result<crate::rb::Symbol, Error> {
    crate::raw::sym(ruby, name)
}

/// [`symbol`] as a plain `Value`.
pub fn sym(ruby: &Ruby, name: &str) -> Result<Value, Error> {
    raw::sym(ruby, name).map(|s| s.as_value())
}

/// A UTF-8 String holding a copy of `bytes`; a failed allocation is `Err`
/// (`NoMemoryError`), as `raw`'s every call is.
///
/// `bytes` must be valid UTF-8 (the String is tagged so without a check):
/// everything a document lends is, by the text-input contract, and so is
/// every checked view. Bytes that were not would make a wrong String, never
/// a memory error.
pub fn utf8_str(ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
    utf8_rstring(ruby, bytes).map(|s| s.as_value())
}

/// [`utf8_str`] as an `RString`.
pub fn utf8_rstring(ruby: &Ruby, bytes: &[u8]) -> Result<crate::rb::RString, Error> {
    crate::raw::utf8_str_new(ruby, bytes)
}
