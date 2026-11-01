//! The C calls of Ruby's that can RAISE, each a wrapper that takes the proof
//! it runs inside [`crate::protect::call`]'s `rb_protect`: a `&Protected`,
//! which only that call's thunk lends. So a raise from one of them comes back
//! to `call`'s caller as [`crate::protect::Caught`] instead of `longjmp`ing
//! over Rust frames, by the type rather than by a lint.
//!
//! Each is the C function's own signature with the token first, and does
//! nothing but call it: what checks a value's kind or a bound before the call,
//! and what makes the answer a typed value, is Nupur's (`nupur::raw`). Which
//! functions raise is the lints' table (`makiri_lints`' `NON_RAISING`; every
//! function not in it is taken to raise), and each one Nupur calls is here.
//!
//! # Safety
//!
//! Every wrapper is an `unsafe fn` for what a raw `VALUE` cannot show: each
//! argument is what the C function takes - a live value of the kind it
//! names, a pointer to memory it may read (or write) for the call. Being
//! inside the protection, and so holding the GVL, is the token's.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]
/* The two that run the caller's Ruby (`rb_funcallv`, `rb_yield`) only pass
 * the call on: the lint names each wrapper by the function it wraps, and the
 * sanctioned call of it is an `allow` where Nupur makes it. */
#![cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "the wrappers pass each call on; its caller is checked"
    )
)]

use core::ffi::{c_int, c_void};

use crate::protect::Protected;
use crate::value::Value;
use crate::{rb_data_type_t, rb_encoding, rb_enumerator_size_func, size_t, ID, VALUE};

/// What a wrapper's argument is to C: a [`Value`] its `VALUE`, a pointer to
/// values a pointer to `VALUE`s (`Value` is `repr(transparent)`), and
/// anything else itself.
trait IntoC<C> {
    fn into_c(self) -> C;
}

impl<T> IntoC<T> for T {
    #[inline(always)]
    fn into_c(self) -> T {
        self
    }
}

impl IntoC<VALUE> for Value {
    #[inline(always)]
    fn into_c(self) -> VALUE {
        self.as_raw()
    }
}

impl IntoC<*const VALUE> for *const Value {
    #[inline(always)]
    fn into_c(self) -> *const VALUE {
        self.cast()
    }
}

impl IntoC<*mut VALUE> for *mut Value {
    #[inline(always)]
    fn into_c(self) -> *mut VALUE {
        self.cast()
    }
}

/// A method's trampoline, as `rb_define_method` takes it: `Value` is
/// `repr(transparent)`, so the pointer is the one C calls.
impl IntoC<Option<unsafe extern "C" fn() -> VALUE>> for Option<unsafe extern "C" fn() -> Value> {
    #[inline(always)]
    fn into_c(self) -> Option<unsafe extern "C" fn() -> VALUE> {
        // SAFETY: the same function pointer; its return is a `VALUE` to C.
        self.map(|f| unsafe {
            core::mem::transmute::<unsafe extern "C" fn() -> Value, unsafe extern "C" fn() -> VALUE>(f)
        })
    }
}

/// What C's answer is to a wrapper's caller: a `VALUE` a [`Value`], anything
/// else itself.
trait FromC<C> {
    /// # Safety
    ///
    /// `c` is what a C call of Ruby's just returned, on its thread.
    unsafe fn from_c(c: C) -> Self;
}

impl<T> FromC<T> for T {
    #[inline(always)]
    unsafe fn from_c(c: T) -> T {
        c
    }
}

impl FromC<VALUE> for Value {
    #[inline(always)]
    unsafe fn from_c(c: VALUE) -> Value {
        // SAFETY: what a C call of Ruby's returned, under the protection: a
        // live value on this frame, or a special constant (`Qundef`).
        unsafe { Value::from_raw(c) }
    }
}

macro_rules! raising {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)?;)*) => {$(
        #[doc = concat!("`", stringify!($name), "`, inside the protection.")]
        #[inline(always)]
        pub unsafe fn $name(_: &Protected, $($arg: $ty),*) $(-> $ret)? {
            // SAFETY: the caller's (the module's contract); the token is the
            // protection and the GVL.
            unsafe { FromC::from_c(rb_sys::$name($(IntoC::into_c($arg)),*)) }
        }
    )*};
}

raising! {
    fn rb_ary_new_capa(capa: core::ffi::c_long) -> Value;
    fn rb_ary_push(ary: Value, elem: Value) -> Value;
    fn rb_big2ll(arg1: Value) -> core::ffi::c_longlong;
    fn rb_big2ull(arg1: Value) -> core::ffi::c_ulonglong;
    fn rb_check_frozen(obj: Value);
    fn rb_check_typeddata(obj: Value, data_type: *const rb_data_type_t) -> *mut c_void;
    fn rb_const_get(space: Value, name: ID) -> Value;
    fn rb_const_set(space: Value, name: ID, val: Value);
    fn rb_data_typed_object_zalloc(klass: Value, size: size_t, type_: *const rb_data_type_t) -> Value;
    fn rb_define_class_under(outer: Value, name: *const core::ffi::c_char, super_: Value) -> Value;
    fn rb_define_method(klass: Value, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> Value>, arity: core::ffi::c_int);
    fn rb_define_module(name: *const core::ffi::c_char) -> Value;
    fn rb_define_module_function(klass: Value, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> Value>, arity: core::ffi::c_int);
    fn rb_define_module_under(outer: Value, name: *const core::ffi::c_char) -> Value;
    fn rb_define_private_method(klass: Value, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> Value>, arity: core::ffi::c_int);
    fn rb_define_singleton_method(obj: Value, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> Value>, arity: core::ffi::c_int);
    fn rb_enc_associate(obj: Value, enc: *const rb_encoding) -> Value;
    fn rb_enc_find_index(name: *const core::ffi::c_char) -> core::ffi::c_int;
    fn rb_enc_from_encoding(enc: *const rb_encoding) -> Value;
    fn rb_enc_interned_str(ptr: *const core::ffi::c_char, len: core::ffi::c_long, enc: *const rb_encoding) -> Value;
    fn rb_enumeratorize_with_size(recv: Value, meth: Value, argc: core::ffi::c_int, argv: *const Value, func: rb_enumerator_size_func) -> Value;
    fn rb_exc_new_str(etype: Value, str_: Value) -> Value;
    fn rb_float_new(d: f64) -> Value;
    fn rb_funcallv(recv: Value, mid: ID, argc: core::ffi::c_int, argv: *const Value) -> Value;
    fn rb_gc_register_mark_object(object: Value);
    fn rb_hash_aset(hash: Value, key: Value, val: Value) -> Value;
    fn rb_hash_lookup2(hash: Value, key: Value, def: Value) -> Value;
    fn rb_hash_new() -> Value;
    fn rb_id2sym(id: ID) -> Value;
    fn rb_include_module(klass: Value, module: Value);
    fn rb_intern(name: *const core::ffi::c_char) -> ID;
    fn rb_intern3(name: *const core::ffi::c_char, len: core::ffi::c_long, enc: *const rb_encoding) -> ID;
    fn rb_ivar_set(obj: Value, name: ID, val: Value) -> Value;
    fn rb_ll2inum(num: core::ffi::c_longlong) -> Value;
    fn rb_module_new() -> Value;
    fn rb_obj_classname(obj: Value) -> *const core::ffi::c_char;
    fn rb_obj_freeze(obj: Value) -> Value;
    fn rb_obj_reveal(obj: Value, klass: Value) -> Value;
    fn rb_range_beg_len(range: Value, begp: *mut core::ffi::c_long, lenp: *mut core::ffi::c_long, len: core::ffi::c_long, err: core::ffi::c_int) -> Value;
    fn rb_range_values(range: Value, begp: *mut Value, endp: *mut Value, exclp: *mut core::ffi::c_int) -> core::ffi::c_int;
    fn rb_str_dup(str_: Value) -> Value;
    fn rb_str_encode(str_: Value, to: Value, ecflags: core::ffi::c_int, ecopts: Value) -> Value;
    fn rb_sym2str(symbol: Value) -> Value;
    fn rb_thread_check_ints();
    fn rb_to_encoding(obj: Value) -> *mut rb_encoding;
    fn rb_ull2inum(num: core::ffi::c_ulonglong) -> Value;
    fn rb_undef_alloc_func(klass: Value);
    fn rb_utf8_str_new(ptr: *const core::ffi::c_char, len: core::ffi::c_long) -> Value;
    fn rb_yield(val: Value) -> Value;
}

/// `rb_exc_raise`, inside the protection: raise `exc`. Raises, by design.
///
/// # Safety
///
/// `exc` is an exception object.
#[inline(always)]
pub unsafe fn rb_exc_raise(_: &Protected, exc: Value) -> ! {
    // SAFETY: the contract; the token is the protection.
    unsafe { rb_sys::rb_exc_raise(exc.as_raw()) }
}

/// `rb_hash_foreach`'s callback: a pair of the Hash's, and the walk's own
/// data. `Value` is `repr(transparent)`, and a pointer is a `VALUE`'s width
/// (`uintptr_t`), so this is the function C calls.
pub type HashEach = unsafe extern "C" fn(key: Value, val: Value, arg: *mut c_void) -> c_int;

/// `rb_hash_foreach`, inside the protection: `each` for every pair, with
/// `arg`.
///
/// # Safety
///
/// `hash` is a Hash; `each` treats `arg` as what it is.
#[inline(always)]
pub unsafe fn rb_hash_foreach(_: &Protected, hash: Value, each: HashEach, arg: *mut c_void) {
    // SAFETY: the same function pointer, as C declares it (see `HashEach`).
    let each = unsafe {
        core::mem::transmute::<HashEach, unsafe extern "C" fn(VALUE, VALUE, VALUE) -> c_int>(each)
    };
    // SAFETY: the contract; the token is the protection. The data travels as
    // C's `VALUE`-wide argument.
    unsafe { rb_sys::rb_hash_foreach(hash.as_raw(), Some(each), arg as VALUE) }
}
