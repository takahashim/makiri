//! What a registered method takes and returns.
//!
//! A method takes `Value`s: Makiri converts no argument (`glue`'s
//! `string_arg` and kin check a type and refuse the rest), so the only
//! [`TryConvert`]s are `Value` itself, `bool` (truthiness, read from the
//! bits), and Makiri's own receiver types, which check a TypedData class. None
//! calls Ruby. A method's result becomes a `Value` through [`IntoValue`],
//! which can allocate and so returns `Err` rather than raising.

use super::error::Error;
use super::value::{ReprValue, Value};

/// A method argument read from its `Value`.
pub trait TryConvert: Sized {
    fn try_convert(v: Value) -> Result<Self, Error>;
}

impl TryConvert for Value {
    #[inline]
    fn try_convert(v: Value) -> Result<Value, Error> {
        Ok(v)
    }
}

impl TryConvert for bool {
    #[inline]
    fn try_convert(v: Value) -> Result<bool, Error> {
        Ok(v.to_bool())
    }
}

/// A method's result, as the `Value` Ruby receives.
pub trait IntoValue {
    fn into_value(self) -> Result<Value, Error>;
}

impl<T: ReprValue> IntoValue for T {
    #[inline]
    fn into_value(self) -> Result<Value, Error> {
        Ok(self.as_value())
    }
}

impl IntoValue for () {
    #[inline]
    fn into_value(self) -> Result<Value, Error> {
        Ok(crate::bridge::ruby::nil())
    }
}

impl IntoValue for bool {
    #[inline]
    fn into_value(self) -> Result<Value, Error> {
        Ok(crate::bridge::ruby::boolean(self))
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    #[inline]
    fn into_value(self) -> Result<Value, Error> {
        match self {
            Some(v) => v.into_value(),
            None => Ok(crate::bridge::ruby::nil()),
        }
    }
}

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            #[inline]
            fn into_value(self) -> Result<Value, Error> {
                crate::bridge::build::int(self as i64)
            }
        }
    )*};
}
signed!(i32, i64);

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            #[inline]
            fn into_value(self) -> Result<Value, Error> {
                crate::bridge::build::uint(self as u64)
            }
        }
    )*};
}
unsigned!(u32, u64, usize);

impl IntoValue for &'static str {
    #[inline]
    fn into_value(self) -> Result<Value, Error> {
        crate::bridge::string::utf8_str(self.as_bytes())
    }
}

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn into_value(self) -> Result<Value, Error> {
        let (a, b) = (self.0.into_value()?, self.1.into_value()?);
        let ary = crate::bridge::build::ary(2)?;
        crate::bridge::build::push(ary, a)?;
        crate::bridge::build::push(ary, b)?;
        Ok(ary.as_value())
    }
}

/// What a registered function returns: a value, or a `Result` of one.
pub trait ReturnValue {
    fn into_return(self) -> Result<Value, Error>;
}

impl<T: IntoValue> ReturnValue for T {
    #[inline]
    fn into_return(self) -> Result<Value, Error> {
        self.into_value()
    }
}

impl<T: IntoValue> ReturnValue for Result<T, Error> {
    #[inline]
    fn into_return(self) -> Result<Value, Error> {
        self?.into_value()
    }
}
