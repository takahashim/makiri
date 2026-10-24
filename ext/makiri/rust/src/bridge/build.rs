//! Ruby objects the glue builds - Arrays, Hashes, Integers, Floats, Symbols -
//! each made through `raw`'s protected calls, so a failed allocation is `Err`.
//!
//! magnus's own constructors (`Ruby::ary_new`, `Ruby::hash_new`,
//! `Ruby::integer_from_i64`, ...) raise instead, a `longjmp` over the caller's
//! frames; `clippy.toml` bans them outside this module and init, so the glue
//! spells every one of these here. Strings are `bridge::string`'s.

#![forbid(unsafe_code)]

use crate::bridge::rb::ReprValue;
use crate::bridge::rb::{Error, RArray, RHash, Value};

use super::raw;
use super::ruby::gvl_ruby;

/// An empty Array with room for `capa` elements.
pub fn ary(capa: usize) -> Result<RArray, Error> {
    raw::ary_new_capa(&gvl_ruby(), capa)
}

/// Append `v` to `a`.
pub fn push(a: RArray, v: Value) -> Result<(), Error> {
    raw::ary_push(a, v)
}

/// An empty Hash.
pub fn hash() -> Result<RHash, Error> {
    raw::hash_new(&gvl_ruby())
}

/// `h[k] = v`, for a String or Symbol `k` (any other is a `TypeError`: see
/// `raw::hash_aset`).
pub fn aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    raw::hash_aset(h, k, v)
}

/// An Integer.
pub fn int(n: i64) -> Result<Value, Error> {
    raw::int_from_i64(&gvl_ruby(), n).map(|i| i.as_value())
}

/// An Integer from an unsigned value.
pub fn uint(n: u64) -> Result<Value, Error> {
    raw::int_from_u64(&gvl_ruby(), n).map(|i| i.as_value())
}

/// A Float.
pub fn float(f: f64) -> Result<Value, Error> {
    raw::float_new(&gvl_ruby(), f).map(|f| f.as_value())
}

/// `obj.enum_for(meth)`: an Enumerator over `obj`'s method `meth`.
pub fn enumerator(obj: Value, meth: &str) -> Result<Value, Error> {
    raw::enumeratorize(obj, raw::sym(&gvl_ruby(), meth)?)
}

/// The Symbol named by the UTF-8 `name`.
pub fn symbol(name: &str) -> Result<crate::bridge::rb::Symbol, Error> {
    crate::bridge::raw::sym(&crate::bridge::ruby::gvl_ruby(), name)
}

/// [`symbol`] as a plain `Value`.
pub fn sym(name: &str) -> Result<Value, Error> {
    raw::sym(&gvl_ruby(), name).map(|s| s.as_value())
}
