//! [`Ruby`]: the proof that this thread holds the GVL.

#![allow(unsafe_code)]

use core::marker::PhantomData;

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
pub struct Ruby(PhantomData<*mut ()>);

macro_rules! globals {
    ($kind:ident: $($method:ident => $global:ident),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($global), "`.")]
            #[inline]
            pub fn $method(&self) -> $kind {
                // SAFETY: a C global Ruby sets once at boot: a live value.
                let v = unsafe { Value::from_raw(nupur_sys::read::$global(self.gvl())) };
                $kind::from_known(v)
            }
        )*
    };
}

impl Ruby {
    /// The token, unchecked: the layer's own, called by the two entries Ruby
    /// calls with the GVL held (`rb::method`'s `finish` and `run_init`) and
    /// nowhere else (`rake dylint`'s `gvl_token_outside_entry`).
    ///
    /// # Safety
    /// On a Ruby thread holding the GVL.
    #[inline]
    pub(crate) unsafe fn get_unchecked() -> Ruby {
        Ruby(PhantomData)
    }

    /// The proof of the GVL the token is, for `nupur_sys`'s reads.
    #[inline(always)]
    pub(crate) fn gvl(&self) -> &nupur_sys::gvl::Gvl {
        // SAFETY: a `Ruby` exists only on a Ruby thread holding the GVL, and
        // the proof is lent for the token's borrow.
        unsafe { nupur_sys::gvl::Gvl::assume() }
    }

    /// `nil`.
    #[inline]
    pub fn qnil(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(nupur_sys::Qnil as nupur_sys::VALUE) }
    }

    /// `true`.
    #[inline]
    pub fn qtrue(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(nupur_sys::Qtrue as nupur_sys::VALUE) }
    }

    /// `false`.
    #[inline]
    pub fn qfalse(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(nupur_sys::Qfalse as nupur_sys::VALUE) }
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
