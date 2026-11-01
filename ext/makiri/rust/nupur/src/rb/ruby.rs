//! [`Ruby`]: the proof that this thread holds the GVL.

#![allow(unsafe_code)]

use super::error::Error;
use super::value::{ExceptionClass, RClass, RModule, Value};

/// The GVL, held: a Ruby method's thread, between its entry and its return.
///
/// A zero-sized token, neither `Send` nor `Sync`, so it cannot be carried into
/// a GVL-released body (`gvl::without_gvl`'s `Send` bound). Only the
/// entries Ruby calls make one - a method's trampoline (`method::finish`) and
/// `Init_` (`method::run_init`) - and everything below them is handed it as
/// `&Ruby`: there is no other way to get one. Making an [`Error`] needs none,
/// since its class is read only when it is raised.
///
/// [`Error`]: super::error::Error
///
/// `nupur_sys`'s token underneath, which its calls that take no value take.
///
/// The field is `rb`'s, so the one maker, `rb::method`'s private
/// `entry_token`, can build one: no other module of the layer can.
pub struct Ruby(pub(super) nupur_sys::value::Ruby);

macro_rules! globals {
    ($kind:ident: $($method:ident => $global:ident),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($global), "`.")]
            #[inline]
            pub fn $method(&self) -> $kind {
                let v = Value::from_sys(nupur_sys::non_raising::$global(self.sys()));
                $kind::from_known(v)
            }
        )*
    };
}

impl Ruby {
    /// `nupur_sys`'s token, for its calls.
    #[inline(always)]
    pub(crate) fn sys(&self) -> &nupur_sys::value::Ruby {
        &self.0
    }

    /// `nil`.
    #[inline]
    pub fn qnil(&self) -> Value {
        Value::from_sys(self.0.qnil())
    }

    /// `true`.
    #[inline]
    pub fn qtrue(&self) -> Value {
        Value::from_sys(self.0.qtrue())
    }

    /// `false`.
    #[inline]
    pub fn qfalse(&self) -> Value {
        Value::from_sys(self.0.qfalse())
    }

    globals! { ExceptionClass:
        exception_exception => rb_eException,
        exception_standard_error => rb_eStandardError,
        exception_runtime_error => rb_eRuntimeError,
        exception_arg_error => rb_eArgError,
        exception_type_error => rb_eTypeError,
        exception_range_error => rb_eRangeError,
        exception_not_imp_error => rb_eNotImpError,
        exception_no_mem_error => rb_eNoMemError,
        exception_encoding_error => rb_eEncodingError,
        exception_load_error => rb_eLoadError,
    }

    globals! { RClass:
        class_encoding => rb_cEncoding,
        class_range => rb_cRange,
        class_string => rb_cString,
    }
}

/* What a token defines: modules, at `Init_` (the classes and methods under
 * them are `class`'s, on the module). */
impl Ruby {
    /// A top-level module.
    pub fn define_module(&self, name: &str) -> Result<RModule, Error> {
        Ok(RModule::from_known(crate::raw::define_module(
            self,
            &super::class::c_name(name)?,
        )?))
    }

    /// An anonymous module.
    pub fn module_new(&self) -> Result<RModule, Error> {
        crate::raw::module_new(self)
    }
}
