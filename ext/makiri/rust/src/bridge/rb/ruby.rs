//! [`Ruby`]: the proof that this thread holds the GVL.

use core::marker::PhantomData;

use super::value::{ExceptionClass, RClass, ReprValue, Value};

/// The GVL, held: a Ruby method's thread, between its entry and its return.
///
/// A zero-sized token, neither `Send` nor `Sync`, so it cannot be carried into
/// a GVL-released body (`bridge::gvl::without_gvl`'s `Send` bound). A method
/// is handed one by its trampoline; anything else gets one from a value
/// (`get_with`, a value existing being proof enough) or, where nothing else
/// is at hand, from `bridge::ruby::gvl_ruby`.
pub struct Ruby(PhantomData<*mut ()>);

macro_rules! globals {
    ($kind:ident: $($method:ident => $global:ident),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($global), "`.")]
            #[inline]
            pub fn $method(&self) -> $kind {
                // SAFETY: a C global Ruby sets once at boot, read under the GVL.
                let v = unsafe { Value::from_raw(rb_sys::$global) };
                $kind::from_known(v)
            }
        )*
    };
}

impl Ruby {
    /// The token, unchecked.
    ///
    /// # Safety
    /// On a Ruby thread holding the GVL.
    #[inline]
    pub unsafe fn get_unchecked() -> Ruby {
        Ruby(PhantomData)
    }

    /// The token, from a value: one exists only where the GVL is held.
    #[inline]
    pub fn get_with(_: impl ReprValue) -> Ruby {
        Ruby(PhantomData)
    }

    /// `nil`.
    #[inline]
    pub fn qnil(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(rb_sys::Qnil as rb_sys::VALUE) }
    }

    /// `true`.
    #[inline]
    pub fn qtrue(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(rb_sys::Qtrue as rb_sys::VALUE) }
    }

    /// `false`.
    #[inline]
    pub fn qfalse(&self) -> Value {
        // SAFETY: an immediate.
        unsafe { Value::from_raw(rb_sys::Qfalse as rb_sys::VALUE) }
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
        class_object => rb_cObject,
        class_encoding => rb_cEncoding,
        class_range => rb_cRange,
    }
}
