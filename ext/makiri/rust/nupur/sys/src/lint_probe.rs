//! What the extension's lints (`lints/makiri_lints`) must report in
//! `nupur_sys`, the crate under the binding layer that alone depends on
//! rb-sys: that the layer's rules reach it too. Compiled only under
//! `--cfg nupur_sys_lint_probe`, by `rake dylint`, which fails unless the
//! lints report exactly the lines that end in `//~`, each by the lints named
//! after it.
#![allow(dead_code, unsafe_code, clippy::all)]

/// A raising C call outside the protection, from a function with no proof
/// of the GVL (`raising_call_outside_protect`, `ffi_without_gvl_proof`).
fn raising() {
    let _ = unsafe { crate::rb_ary_new_capa(0) }; //~ ffi_without_gvl_proof raising_call_outside_protect
}

/// An `unsafe fn` of the crate's, whose contract is the GVL, from a safe
/// function with no proof (`ffi_without_gvl_proof`).
fn sys_contract() {
    unsafe { crate::protect::forget_errinfo() } //~ ffi_without_gvl_proof
}

/// An `extern "C"` function whose body runs under no guard
/// (`unguarded_no_unwind_site`).
extern "C" fn unguarded() {} //~ unguarded_no_unwind_site

/// An integer `as` cast (`c_integer_outside_int`).
fn cast(n: i64) -> i32 {
    n as i32 //~ c_integer_outside_int
}

/// A wrapper of a call that cannot raise, beside the ones that can
/// (`unraising_call_in_raw`): the `raising` module holds only calls that need
/// the protection.
#[rustfmt::skip] // keeps each marker on the line reported
mod raising {
    unsafe fn rb_utf8_encoding(_: &crate::protect::Protected) -> *mut crate::rb_encoding {
        unsafe { rb_sys::rb_utf8_encoding() } //~ unraising_call_in_raw
    }

    unsafe fn rb_ary_new(_: &crate::protect::Protected) -> crate::VALUE {
        unsafe { rb_sys::rb_ary_new() }
    }
}
