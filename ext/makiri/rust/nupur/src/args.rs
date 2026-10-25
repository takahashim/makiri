//! Reading a method's arguments: each a type check, and no conversion - a
//! `to_s`, `to_str` or `to_int` is the caller's Ruby code, which the layer
//! runs none of - so a value of another type is a `TypeError`, worded as
//! Ruby's own type check words it, with the way out.

#![forbid(unsafe_code)]

use core::ffi::c_long;

use crate::describe::ClassName;
use crate::rb::{prelude::*, Builtin, Error, RString, Value};

/// `v`, which must be a String (a subclass's instance included).
///
/// There is no conversion: a `to_str` or `to_s` is the caller's Ruby code, and
/// the layer runs none, so anything else is a `TypeError`, worded as Ruby's own
/// type check words it.
pub fn string_arg(v: Value) -> Result<RString, Error> {
    RString::from_value(v).ok_or_else(|| wrong_type(v, "String", TO_S_HINT))
}

/// `v`, which must be an Integer (a Fixnum or a Bignum). No conversion, as
/// [`string_arg`]: a `to_int` is the caller's Ruby.
pub fn integer_arg(v: Value) -> Result<crate::rb::Integer, Error> {
    crate::rb::Integer::from_value(v).ok_or_else(|| wrong_type(v, "Integer", TO_I_HINT))
}

/// [`integer_arg`] as a `long`; one past a `long` is Ruby's `RangeError`.
pub fn long_arg(v: Value) -> Result<c_long, Error> {
    let n = integer_arg(v)?.to_i64()?;
    c_long::try_from(n).map_err(|_| {
        Error::new(
            Builtin::RangeError,
            format_args!("integer {n} too big for a long"),
        )
    })
}

/// A Range argument whose ends are Integers (or nil, for an endless or
/// beginless one): `rb_range_beg_len` would otherwise convert an end with its
/// `to_int`.
pub fn integer_range_arg(v: Value) -> Result<Value, Error> {
    let r = crate::rb::Range::from_value(v).ok_or_else(|| wrong_type(v, "Range", ""))?;
    let (beg, end, _) = r.values()?;
    for end in [beg, end] {
        if !end.is_nil() {
            integer_arg(end)?;
        }
    }
    Ok(v)
}

const TO_S_HINT: &str = "; call #to_s to convert it";
const TO_I_HINT: &str = "; call #to_i to convert it";

/// `v` as a String for a NAME (an attribute or element name, a prefix, a
/// selector, an expression): a String, or a Symbol's name. Nothing else
/// converts, as [`string_arg`].
pub fn name_arg(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        return Ok(s);
    }
    match crate::rb::Symbol::from_value(v) {
        Some(sym) => super::raw::sym2str(sym),
        None => Err(wrong_type(v, "String or Symbol", TO_S_HINT)),
    }
}

/// Ruby's `TypeError` for a value of the wrong class, worded as Ruby's own
/// type check words it, with the way out (`hint`): "wrong argument type
/// Integer (expected String); call #to_s to convert it". The layer converts
/// nothing itself (a `to_s` is the caller's Ruby), so the caller does. The
/// class name is read without a call.
fn wrong_type(v: Value, expected: &str, hint: &str) -> Error {
    crate::rb::error::type_error(format_args!(
        "wrong argument type {} (expected {expected}){hint}",
        ClassName(v)
    ))
}
