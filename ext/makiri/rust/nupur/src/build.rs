//! Ruby objects made from Rust values - Arrays, Hashes, Integers, Floats,
//! Symbols - each a protected call of `raw`'s, so a failed
//! allocation is `Err`. The one way a caller makes one: `raw`'s constructors
//! are the crate's own. Each answers its own type, which every function
//! taking a value takes as it is (`impl ReprValue`). A String is `string`'s
//! (`string::utf8_str`).

#![forbid(unsafe_code)]

use crate::rb::ReprValue;
use crate::rb::{Error, Float, Integer, RArray, RHash, Ruby, Symbol, Value};

use super::raw;

/// An empty Array with room for `capa` elements.
pub fn ary(ruby: &Ruby, capa: usize) -> Result<RArray, Error> {
    raw::ary_new_capa(ruby, capa)
}

/// Append `v` to `a`.
pub fn push(a: RArray, v: impl ReprValue) -> Result<(), Error> {
    raw::ary_push(a, v.as_value())
}

/// An empty Hash.
pub fn hash(ruby: &Ruby) -> Result<RHash, Error> {
    raw::hash_new(ruby)
}

/// `h[k] = v`, for a String or Symbol `k` (any other is a `TypeError`: see
/// `raw::hash_aset`).
pub fn aset(h: RHash, k: impl ReprValue, v: impl ReprValue) -> Result<(), Error> {
    raw::hash_aset(h, k.as_value(), v.as_value())
}

/// An Integer.
pub fn int(ruby: &Ruby, n: i64) -> Result<Integer, Error> {
    raw::int_from_i64(ruby, n)
}

/// An Integer from an unsigned value.
pub(crate) fn uint(ruby: &Ruby, n: u64) -> Result<Integer, Error> {
    raw::int_from_u64(ruby, n)
}

/// A Float.
pub fn float(ruby: &Ruby, f: f64) -> Result<Float, Error> {
    raw::float_new(ruby, f)
}

/// `obj.enum_for(meth)`: an Enumerator over `obj`'s method `meth`.
pub fn enumerator(ruby: &Ruby, obj: Value, meth: &str) -> Result<Value, Error> {
    raw::enumeratorize(obj, raw::sym(ruby, meth)?)
}

/// The Symbol named by the UTF-8 `name`.
pub fn symbol(ruby: &Ruby, name: &str) -> Result<Symbol, Error> {
    raw::sym(ruby, name)
}
