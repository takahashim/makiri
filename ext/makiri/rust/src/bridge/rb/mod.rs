//! Makiri's Ruby layer over `rb-sys`: the values, errors, method registration
//! and class definitions the glue is written against.
//!
//! It stands where magnus stood, under the names magnus gave them, and holds
//! only what Makiri uses: a `Value` and the class-checked handles over it
//! ([`value`]), [`Error`] ([`error`]), what a method takes and returns
//! ([`convert`]), the trampolines `method!` / `function!` expand to
//! ([`method`]) and class and module definitions ([`class`]). Every C call it
//! makes that can raise is a `bridge::raw` wrapper under `rb_protect`, and
//! nothing raises until a method's trampoline has returned from the method
//! (`method::finish`). So the C functions Makiri trusts are the ones
//! `bridge/raw.rs` names, and no other code stands between them and the glue.

#![allow(unsafe_code)]

pub mod class;
pub mod convert;
pub mod error;
pub mod method;
pub mod ruby;
pub mod value;

pub use class::{freeze, Class, Module, Object, Opaque};
pub use convert::{IntoValue, ReturnValue, TryConvert};
pub use error::{Error, Message};
pub use ruby::Ruby;
pub use value::{
    Coderange, ExceptionClass, Float, HashKey, Integer, RArray, RClass, RHash, RModule, RString,
    Range, ReprValue, Symbol, Value,
};

/// The traits a glue module brings into scope with `use ...::prelude::*`.
pub mod prelude {
    pub use super::{Class, Module, Object, ReprValue, TryConvert};
}
