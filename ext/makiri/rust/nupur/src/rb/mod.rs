//! The value and method layer over `rb-sys`: the values, errors, method registration
//! and class definitions an extension is written against.
//!
//! It stands where magnus stood, under the names magnus gave them, and holds
//! only what is used: a `Value` and the class-checked handles over it
//! ([`value`]), [`Error`] ([`error`]), what a method takes and returns
//! ([`convert`]), the trampolines `method!` / `function!` expand to
//! ([`method`]) and class and module definitions ([`class`]). Every C call it
//! makes that can raise is a `raw` wrapper under `rb_protect`, and
//! nothing raises until a method's trampoline has returned from the method
//! (`method::finish`). So the C functions an extension trusts are the ones
//! `raw.rs` names, and no other code stands between them and its methods.

pub mod class;
pub mod convert;
pub mod error;
pub mod global;
pub(crate) mod kept;
pub mod method;
pub mod object;
pub mod ruby;
pub mod value;

pub use class::{freeze, DataClass, Module, Object};
pub use convert::{IntoValue, ReturnValue, TryConvert};
pub use error::{Builtin, Error, ErrorClass, Message, Rescued};
pub use global::{Opaque, RbConst};
pub use ruby::Ruby;
pub use value::{
    Coderange, ExceptionClass, Float, HashKey, Id, Integer, RArray, RClass, RHash, RModule,
    RString, Range, ReprValue, Symbol, Value,
};

/// The traits a method's module brings into scope with `use ...::prelude::*`.
pub mod prelude {
    pub use super::{Module, Object, ReprValue, TryConvert};
}
