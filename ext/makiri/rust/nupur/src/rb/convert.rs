//! What a registered method takes and returns.
//!
//! A method takes `Value`s: the layer converts no argument (`args`'
//! `string_arg` and kin check a type and refuse the rest), so the only
//! [`TryConvert`]s are `Value` itself, `bool` (truthiness, read from the
//! bits), and an extension's own receiver types, which check a TypedData class. None
//! calls Ruby. A method's result becomes a `Value` through [`IntoValue`],
//! which can allocate and so returns `Err` rather than raising.

#![forbid(unsafe_code)]

use super::error::Error;
use super::ruby::Ruby;
use super::value::{ReprValue, Value};

/// A method argument read from its `Value`.
pub trait TryConvert: Sized {
    fn try_convert(ruby: &Ruby, v: Value) -> Result<Self, Error>;
}

impl TryConvert for Value {
    #[inline]
    fn try_convert(_: &Ruby, v: Value) -> Result<Value, Error> {
        Ok(v)
    }
}

impl TryConvert for bool {
    #[inline]
    fn try_convert(_: &Ruby, v: Value) -> Result<bool, Error> {
        Ok(v.to_bool())
    }
}

/// A method's result, as the `Value` Ruby receives.
pub trait IntoValue {
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error>;
}

impl<T: ReprValue> IntoValue for T {
    #[inline]
    fn into_value(self, _: &Ruby) -> Result<Value, Error> {
        Ok(self.as_value())
    }
}

impl IntoValue for () {
    #[inline]
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        Ok(crate::rb::value::nil(ruby))
    }
}

impl IntoValue for bool {
    #[inline]
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        Ok(crate::rb::value::boolean(ruby, self))
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    #[inline]
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        match self {
            Some(v) => v.into_value(ruby),
            None => Ok(crate::rb::value::nil(ruby)),
        }
    }
}

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            #[inline]
            fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
                crate::build::int(ruby, i64::from(self)).map(ReprValue::as_value)
            }
        }
    )*};
}
signed!(i32, i64);

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            #[inline]
            fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
                crate::build::uint(ruby, u64::from(self)).map(ReprValue::as_value)
            }
        }
    )*};
}
unsigned!(u32, u64);

impl IntoValue for usize {
    #[inline]
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        crate::build::uint(ruby, crate::int::wide(self)).map(ReprValue::as_value)
    }
}

impl IntoValue for &'static str {
    #[inline]
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        crate::string::utf8_str(ruby, self.as_bytes()).map(ReprValue::as_value)
    }
}

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn into_value(self, ruby: &Ruby) -> Result<Value, Error> {
        let (a, b) = (self.0.into_value(ruby)?, self.1.into_value(ruby)?);
        let ary = crate::build::ary(ruby, 2)?;
        crate::build::push(ary, a)?;
        crate::build::push(ary, b)?;
        Ok(ary.as_value())
    }
}

/// What a registered function returns: a value, or a `Result` of one.
pub trait ReturnValue {
    fn into_return(self, ruby: &Ruby) -> Result<Value, Error>;
}

impl<T: IntoValue> ReturnValue for T {
    #[inline]
    fn into_return(self, ruby: &Ruby) -> Result<Value, Error> {
        self.into_value(ruby)
    }
}

impl<T: IntoValue> ReturnValue for Result<T, Error> {
    #[inline]
    fn into_return(self, ruby: &Ruby) -> Result<Value, Error> {
        self?.into_value(ruby)
    }
}
