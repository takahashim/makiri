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

/// A new Array of `vals`, made in one protected call rather than a push
/// each. `vals` is held on the caller's stack, where the GC sees it.
pub fn ary_from(ruby: &Ruby, vals: &[Value]) -> Result<RArray, Error> {
    raw::ary_from_values(ruby, vals)
}

/// Append `vals` to `a`, in one protected call.
pub fn ary_cat(a: RArray, vals: &[Value]) -> Result<(), Error> {
    raw::ary_cat(a, vals)
}

/// `pairs` - key, value, key, value - into `h`, in one protected call, a
/// later key's value replacing an earlier one's. A key must be a String or
/// a Symbol, as for [`aset`].
pub fn hash_insert_pairs(h: RHash, pairs: &[Value]) -> Result<(), Error> {
    raw::hash_bulk_insert(h, pairs)
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

/// The Integer `digits` spell in decimal - an optional `-`, then one or
/// more ASCII digits, nothing else - which may be past an `i64` (a Bignum).
/// Anything else is `ArgumentError`, decided here before Ruby reads them.
/// A value an `i64` holds is made without a String.
pub fn int_from_decimal(ruby: &Ruby, digits: &[u8]) -> Result<Integer, Error> {
    let body = digits.strip_prefix(b"-").unwrap_or(digits);
    if body.is_empty() || !body.iter().all(u8::is_ascii_digit) {
        return Err(crate::rb::error::arg_error("not a decimal integer"));
    }
    /* All ASCII, checked above. */
    if let Some(n) = core::str::from_utf8(digits)
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
    {
        return int(ruby, n);
    }
    raw::str_to_inum(crate::string::binary_str(ruby, digits)?)
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
