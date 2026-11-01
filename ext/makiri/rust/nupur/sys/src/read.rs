//! Ruby's C that raises nothing, each read taking the proof of the GVL
//! ([`Gvl`]): the C functions the lints' table finds no raise in
//! (`makiri_lints`' `NON_RAISING`, and the one `REVIEWED` for any caller),
//! the inline reads of rb-sys's stable API, and the C globals that hold a
//! class.
//!
//! A function that can raise, but raises nothing under a precondition
//! (`rb_enc_get` of a String, `rb_gc_mark` in a mark callback), is here
//! named for that precondition, which its `unsafe` contract states: a
//! caller inherits no reason it has not shown in its own SAFETY.
//!
//! The stable API's lengths come back in Rust's widths (`usize`, `i64`),
//! through [`crate::int`]. The C functions keep their C signatures.
//!
//! # Safety
//!
//! Every function that takes a raw `VALUE` or pointer is an `unsafe fn` for
//! what that cannot show: each argument is what the C function takes - a
//! live value of the kind it names, a pointer to memory it may read (or
//! write, or free) for the call. Holding the GVL is the token's.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use rb_sys::stable_api::{get_default, StableApiDefinition};

use crate::gvl::Gvl;
use crate::{rb_data_type_t, rb_encoding, ruby_value_type, size_t, ssize_t, ID, VALUE};
use rb_sys::rb_unblock_function_t;

macro_rules! reads {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)?;)*) => {$(
        #[doc = concat!("`", stringify!($name), "`, with the GVL.")]
        #[inline(always)]
        pub unsafe fn $name(_: &Gvl, $($arg: $ty),*) $(-> $ret)? {
            // SAFETY: the caller's (the module's contract); the token is the
            // GVL.
            unsafe { rb_sys::$name($($arg),*) }
        }
    )*};
}

reads! {
    fn rb_ary_entry(ary: VALUE, off: core::ffi::c_long) -> VALUE;
    fn rb_ascii8bit_encindex() -> c_int;
    fn rb_block_given_p() -> c_int;
    fn rb_enc_from_index(idx: c_int) -> *mut rb_encoding;
    fn rb_enc_to_index(enc: *const rb_encoding) -> c_int;
    fn rb_ext_ractor_safe(flag: bool);
    fn rb_gc_adjust_memory_usage(diff: ssize_t);
    fn rb_keyword_given_p() -> c_int;
    fn rb_obj_hide(obj: VALUE) -> VALUE;
    fn rb_thread_call_without_gvl2(
        func: Option<unsafe extern "C" fn(arg1: *mut c_void) -> *mut c_void>,
        data1: *mut c_void,
        ubf: rb_unblock_function_t,
        data2: *mut c_void
    ) -> *mut c_void;
    fn rb_typeddata_is_kind_of(obj: VALUE, data_type: *const rb_data_type_t) -> c_int;
    fn rb_usascii_encindex() -> c_int;
    fn rb_utf8_encindex() -> c_int;
    fn rb_utf8_encoding() -> *mut rb_encoding;
    fn ruby_xfree(ptr: *mut c_void);
}

/// `ruby_native_thread_p`: whether this thread is one Ruby made. The one
/// read that needs no proof - it is the question of whether there is one -
/// and is safe from any thread.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "asks whether this is a Ruby thread at all, which any thread may ask"
    )
)]
pub fn ruby_native_thread_p() -> bool {
    // SAFETY: reads the thread-local Ruby keeps for its own threads, and
    // answers 0 on any other.
    unsafe { rb_sys::ruby_native_thread_p() != 0 }
}

/// Whether the stack is nearly spent (`ruby_stack_check`), on a thread Ruby
/// runs - with the GVL or without it, as each reads its own stack's bounds.
/// A thread Ruby does not run has no execution context to read, and is
/// answered "room left", as a build with no probe answers. Safe from any
/// thread, so it takes no proof.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "reads the stack only on a thread Ruby made, with the GVL or without it, as `ruby_native_thread_p` answers first"
    )
)]
pub fn ruby_stack_low() -> bool {
    // SAFETY: `ruby_stack_check` runs only on a thread Ruby made, where it
    // takes no argument, raises nothing, and reads that thread's bounds.
    ruby_native_thread_p() && unsafe { rb_sys::ruby_stack_check() != 0 }
}

/* ---- the stable API's inline reads ---- */

/// `RB_NIL_P`.
#[inline(always)]
pub fn nil_p(_: &Gvl, v: VALUE) -> bool {
    get_default().nil_p(v)
}

/// `RB_TEST`: Ruby's truthiness.
#[inline(always)]
pub fn rb_test(_: &Gvl, v: VALUE) -> bool {
    get_default().rb_test(v)
}

/// `RB_SPECIAL_CONST_P`: an immediate.
#[inline(always)]
pub fn special_const_p(_: &Gvl, v: VALUE) -> bool {
    get_default().special_const_p(v)
}

/// `RB_FIXNUM_P`.
#[inline(always)]
pub fn fixnum_p(_: &Gvl, v: VALUE) -> bool {
    get_default().fixnum_p(v)
}

/// `RB_FIX2LONG`, in Rust's width: `v` must be a Fixnum.
#[inline(always)]
pub fn fix2long(_: &Gvl, v: VALUE) -> i64 {
    i64::from(get_default().fix2long(v))
}

/// `RB_OBJ_FROZEN` of a heap object.
#[inline(always)]
pub unsafe fn frozen_p(_: &Gvl, v: VALUE) -> bool {
    // SAFETY: the caller's: a live, non-immediate value.
    unsafe { get_default().frozen_p(v) }
}

/// `rb_type`.
#[inline(always)]
pub unsafe fn rb_type(_: &Gvl, v: VALUE) -> ruby_value_type {
    // SAFETY: the caller's: a live value.
    unsafe { get_default().rb_type(v) }
}

/// `RB_SYMBOL_P`.
#[inline(always)]
pub unsafe fn symbol_p(_: &Gvl, v: VALUE) -> bool {
    // SAFETY: the caller's: a live value.
    unsafe { get_default().symbol_p(v) }
}

/// `RB_INTEGER_TYPE_P`.
#[inline(always)]
pub unsafe fn integer_type_p(_: &Gvl, v: VALUE) -> bool {
    // SAFETY: the caller's: a live value.
    unsafe { get_default().integer_type_p(v) }
}

/// `RB_FLOAT_TYPE_P`.
#[inline(always)]
pub unsafe fn float_type_p(_: &Gvl, v: VALUE) -> bool {
    // SAFETY: the caller's: a live value.
    unsafe { get_default().float_type_p(v) }
}

/// `RBIGNUM_NEGATIVE_P`.
#[inline(always)]
pub unsafe fn bignum_negative_p(_: &Gvl, v: VALUE) -> bool {
    // SAFETY: the caller's: a live Bignum.
    unsafe { get_default().bignum_negative_p(v) }
}

/// `RSTRING_LEN`, as a `usize`.
#[inline(always)]
pub unsafe fn rstring_len(_: &Gvl, v: VALUE) -> usize {
    // SAFETY: the caller's: a live String.
    crate::int::len(unsafe { get_default().rstring_len(v) })
}

/// `RSTRING_PTR`.
#[inline(always)]
pub unsafe fn rstring_ptr(_: &Gvl, v: VALUE) -> *const core::ffi::c_char {
    // SAFETY: the caller's: a live String.
    unsafe { get_default().rstring_ptr(v) }
}

/// `RARRAY_LEN`, as a `usize`.
#[inline(always)]
pub unsafe fn rarray_len(_: &Gvl, v: VALUE) -> usize {
    // SAFETY: the caller's: a live Array.
    crate::int::len(unsafe { get_default().rarray_len(v) })
}

/// `RHASH_SIZE`.
#[inline(always)]
pub unsafe fn rhash_size(_: &Gvl, v: VALUE) -> usize {
    // SAFETY: the caller's: a live Hash.
    unsafe { get_default().rhash_size(v) }
}

/// `NUM2DBL` of a Float.
#[inline(always)]
pub unsafe fn num2dbl(_: &Gvl, v: VALUE) -> f64 {
    // SAFETY: the caller's: a live Float.
    unsafe { get_default().num2dbl(v) }
}

/// `RTYPEDDATA_GET_DATA`.
#[inline(always)]
pub unsafe fn rtypeddata_get_data(_: &Gvl, v: VALUE) -> *mut c_void {
    // SAFETY: the caller's: a live TypedData object.
    unsafe { get_default().rtypeddata_get_data(v) }
}

/* ---- the C globals that hold a class ---- */

macro_rules! globals {
    ($($name:ident,)*) => {$(
        #[doc = concat!("`", stringify!($name), "`, a C global Ruby sets once at boot.")]
        #[inline(always)]
        #[allow(non_snake_case)]
        pub fn $name(_: &Gvl) -> VALUE {
            // SAFETY: set at boot and never again; a Ruby thread (the token)
            // runs after boot.
            unsafe { rb_sys::$name }
        }
    )*};
}

globals! {
    rb_cEncoding,
    rb_cObject,
    rb_cRange,
    rb_cString,
    rb_eArgError,
    rb_eEncodingError,
    rb_eException,
    rb_eLoadError,
    rb_eNoMemError,
    rb_eNotImpError,
    rb_eRangeError,
    rb_eRuntimeError,
    rb_eStandardError,
    rb_eTypeError,
}

/* ---- the C calls that raise, but not under their precondition ---- */
/*
 * Each of these C functions can raise, and raises nothing under one
 * precondition, which the wrapper's name and contract state: the reason is
 * the precondition's, so a caller that upholds it (its `unsafe` block's
 * SAFETY says how) inherits nothing it has not shown. The `allow` is at the
 * one call of the C function, here.
 */

/// `rb_enc_get` of a String: the encoding it is tagged with.
///
/// # Safety
///
/// `str` is a live String (not a File, whose encoding is a method call, nor
/// a Symbol, whose is a String made).
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_enc_get raises for a File (a method call) or a Symbol (a String made); the contract is a String, whose tag it reads"
    )
)]
pub unsafe fn enc_get_of_string(_: &Gvl, str: VALUE) -> *mut rb_encoding {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_enc_get(str) }
}

/// `rb_enc_get_index` of a String: its encoding's index, looked up past
/// the ones its flags hold inline.
///
/// # Safety
///
/// `str` is a live String.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_enc_get_index raises for a File or a Symbol, as rb_enc_get; the contract is a String, whose index is an ivar read when it is not inline"
    )
)]
pub unsafe fn enc_get_index_of_string(_: &Gvl, str: VALUE) -> c_int {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_enc_get_index(str) }
}

/// `rb_attr_get` of a hidden ivar: one whose name has no `@`.
///
/// # Safety
///
/// `obj` is a live object, and `id` a name without `@` (`rb_is_instance_id`
/// is false for it).
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_attr_get raises only RactorIsolationError, and only for an @ name (rb_is_instance_id) of a class, a module or a shareable object read from another Ractor; the contract is a name without @"
    )
)]
pub unsafe fn attr_get_hidden(_: &Gvl, obj: VALUE, id: ID) -> VALUE {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_attr_get(obj, id) }
}

/// `rb_obj_is_kind_of` of a class or module: an instance of `module` or of
/// one deriving from it? Runs no Ruby code.
///
/// # Safety
///
/// `v` is a live value, and `module` a live class or module.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "its one raise, a non-class, is ruled out by the contract"
    )
)]
pub unsafe fn obj_is_kind_of_module(_: &Gvl, v: VALUE, module: VALUE) -> bool {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_obj_is_kind_of(v, module) == rb_sys::Qtrue as VALUE }
}

/// `rb_class_inherited_p` of two classes or modules: is `sub` `sup` or a
/// descendant of it? Runs no Ruby code.
///
/// # Safety
///
/// `sub` and `sup` are live classes or modules.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "its one raise, a non-class, is ruled out by the contract"
    )
)]
pub unsafe fn class_inherited_p_of_modules(_: &Gvl, sub: VALUE, sup: VALUE) -> bool {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_class_inherited_p(sub, sup) == rb_sys::Qtrue as VALUE }
}

/// `rb_absint_size` of an Integer: the bytes its magnitude takes. Reads its
/// digits, converts nothing.
///
/// # Safety
///
/// `v` is a live Integer.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "its raise is for a non-Integer, converted by `#to_int`; the contract is an Integer"
    )
)]
pub unsafe fn absint_size_of_integer(_: &Gvl, v: VALUE) -> size_t {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_absint_size(v, core::ptr::null_mut()) }
}

/// `rb_str_set_len(str, 0)` of an unfrozen String: empty it.
///
/// # Safety
///
/// `str` is a live, unfrozen String.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_str_set_len raises for a frozen String or a length past its room; the contract is unfrozen, and 0 is within any"
    )
)]
pub unsafe fn str_empty_unfrozen(_: &Gvl, str: VALUE) {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_str_set_len(str, 0) }
}

/// `rb_gc_mark`, from a TypedData type's mark callback.
///
/// # Safety
///
/// Called from a `dmark` callback in Ruby's mark phase, on a live value (or
/// `Qfalse`, which a mark ignores), with nothing on the frames above it that
/// a `longjmp` would leak.
#[inline(always)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_gc_mark raises NoMemoryError only when the mark stack cannot grow, a longjmp the contract's frame loses nothing to and every C extension's dmark is exposed to alike"
    )
)]
pub unsafe fn gc_mark_in_dmark(_: &Gvl, v: VALUE) {
    // SAFETY: the contract.
    unsafe { rb_sys::rb_gc_mark(v) }
}

/// The running Ruby's API `MAJOR.MINOR`, read from the `ruby_api_version`
/// it exports (three ints, whose layout no Ruby changes). Needs no proof:
/// libruby is loaded before any of this runs.
#[inline]
pub fn running_api_version() -> (i64, i64) {
    // SAFETY: an immutable `const int[3]` libruby exports, read by value.
    let (major, minor) = unsafe { (rb_sys::ruby_api_version[0], rb_sys::ruby_api_version[1]) };
    (i64::from(major), i64::from(minor))
}
