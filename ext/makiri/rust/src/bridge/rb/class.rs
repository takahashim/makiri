//! Defining classes, modules and methods - what `Init_makiri` does.
//!
//! Each definition is a C call that can raise (a name already bound to
//! something else, a frozen target), so each goes through `bridge::raw`'s
//! protected wrappers and comes back as `Err`.

use core::ffi::CStr;

use super::error::Error;
use super::method::Func;
use super::ruby::Ruby;
use super::value::{ExceptionClass, RClass, RModule, ReprValue, Value};

/// The longest name a definition takes, its NUL included. Every name Makiri
/// defines is a short Ruby identifier.
const NAME_MAX: usize = 64;

/// A name as the C string the definition functions take, on the stack.
struct CName([u8; NAME_MAX]);

impl core::ops::Deref for CName {
    type Target = CStr;
    fn deref(&self) -> &CStr {
        /* `c_name` wrote a NUL and checked there is no other. */
        CStr::from_bytes_until_nul(&self.0).unwrap_or(c"")
    }
}

/// `name` as a [`CName`]; a name with a NUL, or one too long, is refused.
fn c_name(name: &str) -> Result<CName, Error> {
    let mut buf = [0u8; NAME_MAX];
    match buf.get_mut(..name.len()) {
        Some(dst) if name.len() < NAME_MAX && !name.as_bytes().contains(&0) => {
            dst.copy_from_slice(name.as_bytes());
            Ok(CName(buf))
        }
        _ => Err(crate::bridge::ruby::arg_error(
            "a name to define that is too long or holds a NUL byte",
        )),
    }
}

/// What a class and a module both are: a namespace that holds constants,
/// methods and other classes.
pub trait Module: ReprValue {
    /// `class name < superclass` under this module.
    fn define_class(self, name: &str, superclass: RClass) -> Result<RClass, Error> {
        let v =
            crate::bridge::raw::define_class_under(self.as_value(), &c_name(name)?, superclass)?;
        Ok(RClass::from_known(v))
    }

    /// An exception class `name < superclass` under this module.
    fn define_error(self, name: &str, superclass: ExceptionClass) -> Result<ExceptionClass, Error> {
        let sup = RClass::from_known(superclass.as_value());
        let v = crate::bridge::raw::define_class_under(self.as_value(), &c_name(name)?, sup)?;
        Ok(ExceptionClass::from_known(v))
    }

    /// `module name` under this module.
    fn define_module(self, name: &str) -> Result<RModule, Error> {
        let v = crate::bridge::raw::define_module_under(self.as_value(), &c_name(name)?)?;
        Ok(RModule::from_known(v))
    }

    /// The constant `name` = `value`.
    fn const_set(self, name: &str, value: impl ReprValue) -> Result<(), Error> {
        crate::bridge::raw::const_set(self.as_value(), &c_name(name)?, value.as_value())
    }

    /// A public instance method.
    fn define_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::bridge::raw::define_method(self.as_value(), &c_name(name)?, func, Visibility::Public)
    }

    /// A private instance method.
    fn define_private_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::bridge::raw::define_method(
            self.as_value(),
            &c_name(name)?,
            func,
            Visibility::Private,
        )
    }

    /// A module function: a private instance method and a singleton method.
    fn define_module_function(self, name: &str, func: Func) -> Result<(), Error> {
        crate::bridge::raw::define_method(
            self.as_value(),
            &c_name(name)?,
            func,
            Visibility::ModuleFunction,
        )
    }

    /// The constant `name`, which Makiri's own Ruby defines.
    fn const_get(self, name: &str) -> Result<Value, Error> {
        crate::bridge::raw::const_get(self.as_value(), &c_name(name)?)
    }

    /// `self.name(*args)`: the caller's Ruby - see `bridge::raw::call_method`.
    fn call_method(self, name: &str, args: &[Value]) -> Result<Value, Error> {
        crate::bridge::raw::call_method(self.as_value(), &c_name(name)?, args)
    }

    /// Include `module`.
    fn include_module(self, module: RModule) -> Result<(), Error> {
        crate::bridge::raw::include_module(self.as_value(), module.as_value())
    }
}

/// What every object can have: a singleton method.
pub trait Object: ReprValue {
    /// A method on this object alone - a class method, for a class.
    fn define_singleton_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::bridge::raw::define_method(
            self.as_value(),
            &c_name(name)?,
            func,
            Visibility::Singleton,
        )
    }
}

/// What only a class has: an allocator.
pub trait Class: ReprValue {
    /// Take the allocator away, so `.new` on the class raises: its objects
    /// are made only from Rust, wrapping something.
    fn undef_default_alloc_func(self) {
        crate::bridge::raw::undef_alloc_func(RClass::from_known(self.as_value()));
    }
}

/// How a method is defined.
#[derive(Clone, Copy)]
pub(in crate::bridge) enum Visibility {
    Public,
    Private,
    Singleton,
    ModuleFunction,
}

impl Module for RClass {}
impl Module for RModule {}
impl Object for RClass {}
impl Object for RModule {}
impl Class for RClass {}

impl Ruby {
    /// A top-level module.
    pub fn define_module(&self, name: &str) -> Result<RModule, Error> {
        Ok(RModule::from_known(crate::bridge::raw::define_module(
            &c_name(name)?,
        )?))
    }

    /// An anonymous module.
    pub fn module_new(&self) -> Result<RModule, Error> {
        crate::bridge::raw::module_new(self)
    }
}

/// A value kept for the rest of the process: registered with the GC as a
/// mark root, so it is never collected or moved, and readable from any
/// thread that holds the GVL (`Ruby::get_inner`). What the class table in
/// `init` is made of.
#[derive(Clone, Copy)]
pub struct Opaque<T>(T);

// SAFETY: an `Opaque` is read only through `Ruby::get_inner`, which takes the
// GVL's proof; the value itself is pinned and rooted by `register`.
unsafe impl<T> Send for Opaque<T> {}
// SAFETY: as `Send`.
unsafe impl<T> Sync for Opaque<T> {}

impl<T: ReprValue> Opaque<T> {
    /// `v`, rooted with the GC for the rest of the process.
    pub fn register(v: T) -> Result<Opaque<T>, Error> {
        crate::bridge::raw::gc_register_mark_object(v.as_value())?;
        Ok(Opaque(v))
    }
}

impl Ruby {
    /// The value an [`Opaque`] keeps.
    #[inline]
    pub fn get_inner<T: Copy>(&self, o: Opaque<T>) -> T {
        o.0
    }
}

/// Freeze `v` (`rb_obj_freeze`). Raises nothing for a String of ours.
pub fn freeze(v: Value) {
    crate::bridge::raw::obj_freeze(v);
}
