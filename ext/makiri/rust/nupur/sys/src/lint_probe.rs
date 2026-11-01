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
    let _ = unsafe { rb_sys::rb_ary_new_capa(0) }; //~ ffi_without_gvl_proof raising_call_outside_protect
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
    unsafe fn rb_utf8_encoding(_: &crate::protect::Protected) -> *mut rb_sys::rb_encoding {
        unsafe { rb_sys::rb_utf8_encoding() } //~ unraising_call_in_raw
    }

    unsafe fn rb_ary_new(_: &crate::protect::Protected) -> rb_sys::VALUE {
        unsafe { rb_sys::rb_ary_new() }
    }
}

/// Ruby's C API from a function holding no proof of the GVL
/// (`ffi_without_gvl_proof`) - and the same with one, which passes.
#[rustfmt::skip] // keeps each marker on the line reported
mod gvl_proof {
    fn without_proof() -> bool {
        unsafe { rb_sys::rb_block_given_p() != 0 } //~ ffi_without_gvl_proof
    }

    fn with_the_proof(_: &crate::value::Ruby) -> bool {
        unsafe { rb_sys::rb_block_given_p() != 0 }
    }

    fn in_a_closure() -> bool {
        (|| unsafe { rb_sys::rb_block_given_p() != 0 })() //~ ffi_without_gvl_proof
    }
}

/// Which C functions raise (`raising_call_outside_protect`): one nobody has
/// classified is taken to raise - `rb_undef_alloc_func`, which a list of the
/// raising ones did not name - and one the table says raises nothing passes.
#[rustfmt::skip] // keeps each marker on the line reported
mod classified {
    fn unclassified(_: &crate::value::Ruby) {
        unsafe { rb_sys::rb_undef_alloc_func(rb_sys::rb_cObject) } //~ raising_call_outside_protect
    }

    fn classified(_: &crate::value::Ruby) -> bool {
        unsafe { rb_sys::rb_block_given_p() != 0 }
    }
}
