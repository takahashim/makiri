//! What the extension's lints (`lints/makiri_lints`) must report in the
//! binding layer, the cases only code that names rb-sys can write - the
//! extension cannot, since it does not depend on it. Compiled only under
//! `--cfg nupur_lint_probe`, by `rake dylint`, which fails unless the lints
//! fire on exactly the lines that end in `//~`.
#![allow(dead_code, clippy::all)]

/// A raising C call outside `raw::call`'s closure
/// (`raising_call_outside_protect`).
#[allow(unsafe_code)]
fn raising() {
    let _ = unsafe { rb_sys::rb_ary_new_capa(0) }; //~
}

/// A raw `VALUE` outside the files that are the typed values
/// (`raw_ruby_outside_bridge`).
fn raw_type(_: rb_sys::VALUE) {} //~

/// Ruby's C API from a function holding no proof of the GVL
/// (`ffi_without_gvl_proof`) - and the same with one, which passes.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod gvl_proof {
    fn without_proof() -> bool {
        unsafe { rb_sys::ruby_stack_check() != 0 } //~
    }

    fn with_a_token(_: &crate::rb::Ruby) -> bool {
        unsafe { rb_sys::ruby_stack_check() != 0 }
    }

    fn with_a_value(_: crate::rb::Value) -> bool {
        unsafe { rb_sys::ruby_stack_check() != 0 }
    }

    fn in_a_closure() -> bool {
        (|| unsafe { rb_sys::ruby_stack_check() != 0 })() //~
    }
}
