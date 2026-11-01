//! Defining classes, modules and methods - what an extension's `Init_` does.
//!
//! Each definition is a C call that can raise (a name already bound to
//! something else, a frozen target), so each goes through `raw`'s
//! protected wrappers and comes back as `Err`.

#![forbid(unsafe_code)]

use super::error::Error;
use super::method::Func;
use super::ruby::Ruby;
use super::value::{ExceptionClass, RClass, RModule, ReprValue, Value};
use crate::cname::CName;

/// `name` as the C string a definition takes; a name with a NUL, or one too
/// long, is refused.
pub(crate) fn c_name(name: &str) -> Result<CName, Error> {
    CName::new(name.as_bytes()).ok_or_else(|| {
        crate::rb::error::arg_error("a name to define that is too long or holds a NUL byte")
    })
}

/// What a class and a module both are: a namespace that holds constants,
/// methods and other classes.
pub trait Module: ReprValue {
    /// `class name < superclass` under this module.
    fn define_class(self, name: &str, superclass: RClass) -> Result<RClass, Error> {
        let v = crate::raw::define_class_under(self.as_value(), &c_name(name)?, superclass)?;
        Ok(RClass::from_known(v))
    }

    /// A class `name` under this module whose instances are the layer's
    /// wrapped objects ([`DataClass`]): its superclass is `superclass`, or
    /// `Object` for `None`, and its allocator is taken away, so `.new` makes
    /// none of a kind no payload backs.
    fn define_data_class(
        self,
        name: &str,
        superclass: Option<DataClass>,
    ) -> Result<DataClass, Error> {
        let sup = superclass.map_or_else(|| crate::rb::value::object_class(&self), |d| d.0);
        let v = crate::raw::define_class_under(self.as_value(), &c_name(name)?, sup)?;
        let class = RClass::from_known(v);
        crate::raw::undef_alloc_func(class)?;
        Ok(DataClass(class))
    }

    /// An exception class `name < superclass` under this module.
    fn define_error(self, name: &str, superclass: ExceptionClass) -> Result<ExceptionClass, Error> {
        let sup = RClass::from_known(superclass.as_value());
        let v = crate::raw::define_class_under(self.as_value(), &c_name(name)?, sup)?;
        Ok(ExceptionClass::from_known(v))
    }

    /// `module name` under this module.
    fn define_module(self, name: &str) -> Result<RModule, Error> {
        let v = crate::raw::define_module_under(self.as_value(), &c_name(name)?)?;
        Ok(RModule::from_known(v))
    }

    /// The constant `name` = `value`.
    fn const_set(self, ruby: &Ruby, name: &str, value: impl ReprValue) -> Result<(), Error> {
        crate::raw::const_set(ruby, self.as_value(), &c_name(name)?, value.as_value())
    }

    /// A public instance method.
    fn define_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::raw::define_method(self.as_value(), &c_name(name)?, func, Visibility::Public)
    }

    /// A private instance method.
    fn define_private_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::raw::define_method(self.as_value(), &c_name(name)?, func, Visibility::Private)
    }

    /// A module function: a private instance method and a singleton method.
    fn define_module_function(self, name: &str, func: Func) -> Result<(), Error> {
        crate::raw::define_method(
            self.as_value(),
            &c_name(name)?,
            func,
            Visibility::ModuleFunction,
        )
    }

    /// The constant `name`, which the extension's own Ruby defines.
    fn const_get(self, ruby: &Ruby, name: &str) -> Result<Value, Error> {
        crate::raw::const_get(ruby, self.as_value(), &c_name(name)?)
    }

    /// `self.name(*args)`: the caller's Ruby - see `raw::call_method`.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            runs_callers_ruby,
            reason = "the one way to call a method; each caller's own allow says why"
        )
    )]
    fn call_method(self, ruby: &Ruby, name: &str, args: &[Value]) -> Result<Value, Error> {
        crate::raw::call_method(ruby, self.as_value(), &c_name(name)?, args)
    }

    /// Include `module`.
    fn include_module(self, module: RModule) -> Result<(), Error> {
        crate::raw::include_module(self.as_value(), module.as_value())
    }
}

/// What every object can have: a singleton method.
pub trait Object: ReprValue {
    /// A method on this object alone - a class method, for a class.
    fn define_singleton_method(self, name: &str, func: Func) -> Result<(), Error> {
        crate::raw::define_method(self.as_value(), &c_name(name)?, func, Visibility::Singleton)
    }
}

/// A class made by [`Module::define_data_class`]: one whose instances the
/// layer makes, wrapping a payload ([`crate::typed`] takes no other class).
///
/// Its ancestors are `Object` and such classes only, so no instance of it is
/// handed to a C method that assumes another layout - a T_DATA object made
/// with String's class would have `#length` read it as a String. There is no
/// way to make one from an arbitrary class: no `from_value`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct DataClass(RClass);

impl ReprValue for DataClass {
    #[inline]
    fn as_value(self) -> Value {
        self.0.as_value()
    }
}

impl Module for DataClass {}
impl Object for DataClass {}

/// How a method is defined.
#[derive(Clone, Copy)]
pub(crate) enum Visibility {
    Public,
    Private,
    Singleton,
    ModuleFunction,
}

impl Module for RClass {}
impl Module for RModule {}
impl Object for RClass {}
impl Object for RModule {}

/// Freeze `v` (`rb_obj_freeze`); a failed allocation of its frozen shape is
/// `Err` (`NoMemoryError`).
pub fn freeze(v: impl ReprValue) -> Result<(), Error> {
    crate::raw::obj_freeze(v.as_value())
}
