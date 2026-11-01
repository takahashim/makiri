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

use core::ffi::c_void;

use crate::protect::Protected;
use crate::{rb_data_type_t, rb_encoding, rb_enumerator_size_func, size_t, ID, VALUE};

macro_rules! raising {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)?;)*) => {$(
        #[doc = concat!("`", stringify!($name), "`, inside the protection.")]
        #[inline(always)]
        pub unsafe fn $name(_: &Protected, $($arg: $ty),*) $(-> $ret)? {
            // SAFETY: the caller's (the module's contract); the token is the
            // protection and the GVL.
            unsafe { rb_sys::$name($($arg),*) }
        }
    )*};
}

raising! {
    fn rb_ary_new_capa(capa: core::ffi::c_long) -> VALUE;
    fn rb_ary_push(ary: VALUE, elem: VALUE) -> VALUE;
    fn rb_big2ll(arg1: VALUE) -> core::ffi::c_longlong;
    fn rb_big2ull(arg1: VALUE) -> core::ffi::c_ulonglong;
    fn rb_check_frozen(obj: VALUE);
    fn rb_check_typeddata(obj: VALUE, data_type: *const rb_data_type_t) -> *mut c_void;
    fn rb_const_get(space: VALUE, name: ID) -> VALUE;
    fn rb_const_set(space: VALUE, name: ID, val: VALUE);
    fn rb_data_typed_object_zalloc(klass: VALUE, size: size_t, type_: *const rb_data_type_t) -> VALUE;
    fn rb_define_class_under(outer: VALUE, name: *const core::ffi::c_char, super_: VALUE) -> VALUE;
    fn rb_define_method(klass: VALUE, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> VALUE>, arity: core::ffi::c_int);
    fn rb_define_module(name: *const core::ffi::c_char) -> VALUE;
    fn rb_define_module_function(klass: VALUE, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> VALUE>, arity: core::ffi::c_int);
    fn rb_define_module_under(outer: VALUE, name: *const core::ffi::c_char) -> VALUE;
    fn rb_define_private_method(klass: VALUE, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> VALUE>, arity: core::ffi::c_int);
    fn rb_define_singleton_method(obj: VALUE, mid: *const core::ffi::c_char, func: Option<unsafe extern "C" fn() -> VALUE>, arity: core::ffi::c_int);
    fn rb_enc_associate(obj: VALUE, enc: *const rb_encoding) -> VALUE;
    fn rb_enc_find_index(name: *const core::ffi::c_char) -> core::ffi::c_int;
    fn rb_enc_from_encoding(enc: *const rb_encoding) -> VALUE;
    fn rb_enc_interned_str(ptr: *const core::ffi::c_char, len: core::ffi::c_long, enc: *const rb_encoding) -> VALUE;
    fn rb_enumeratorize_with_size(recv: VALUE, meth: VALUE, argc: core::ffi::c_int, argv: *const VALUE, func: rb_enumerator_size_func) -> VALUE;
    fn rb_exc_new_str(etype: VALUE, str_: VALUE) -> VALUE;
    fn rb_exc_raise(exc: VALUE) -> !;
    fn rb_float_new(d: f64) -> VALUE;
    fn rb_funcallv(recv: VALUE, mid: ID, argc: core::ffi::c_int, argv: *const VALUE) -> VALUE;
    fn rb_gc_register_mark_object(object: VALUE);
    fn rb_hash_aset(hash: VALUE, key: VALUE, val: VALUE) -> VALUE;
    fn rb_hash_foreach(hash: VALUE, func: Option<unsafe extern "C" fn(key: VALUE, val: VALUE, arg: VALUE) -> core::ffi::c_int>, arg: VALUE);
    fn rb_hash_lookup2(hash: VALUE, key: VALUE, def: VALUE) -> VALUE;
    fn rb_hash_new() -> VALUE;
    fn rb_id2sym(id: ID) -> VALUE;
    fn rb_include_module(klass: VALUE, module: VALUE);
    fn rb_intern(name: *const core::ffi::c_char) -> ID;
    fn rb_intern3(name: *const core::ffi::c_char, len: core::ffi::c_long, enc: *const rb_encoding) -> ID;
    fn rb_ivar_set(obj: VALUE, name: ID, val: VALUE) -> VALUE;
    fn rb_ll2inum(num: core::ffi::c_longlong) -> VALUE;
    fn rb_module_new() -> VALUE;
    fn rb_obj_classname(obj: VALUE) -> *const core::ffi::c_char;
    fn rb_obj_freeze(obj: VALUE) -> VALUE;
    fn rb_obj_reveal(obj: VALUE, klass: VALUE) -> VALUE;
    fn rb_range_beg_len(range: VALUE, begp: *mut core::ffi::c_long, lenp: *mut core::ffi::c_long, len: core::ffi::c_long, err: core::ffi::c_int) -> VALUE;
    fn rb_range_values(range: VALUE, begp: *mut VALUE, endp: *mut VALUE, exclp: *mut core::ffi::c_int) -> core::ffi::c_int;
    fn rb_str_dup(str_: VALUE) -> VALUE;
    fn rb_str_encode(str_: VALUE, to: VALUE, ecflags: core::ffi::c_int, ecopts: VALUE) -> VALUE;
    fn rb_sym2str(symbol: VALUE) -> VALUE;
    fn rb_thread_check_ints();
    fn rb_to_encoding(obj: VALUE) -> *mut rb_encoding;
    fn rb_ull2inum(num: core::ffi::c_ulonglong) -> VALUE;
    fn rb_undef_alloc_func(klass: VALUE);
    fn rb_utf8_str_new(ptr: *const core::ffi::c_char, len: core::ffi::c_long) -> VALUE;
    fn rb_yield(val: VALUE) -> VALUE;
}
