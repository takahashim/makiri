//! What the extension's lints (`lints/makiri_lints`) must report in the
//! binding layer, the cases only code that names rb-sys can write - the
//! extension cannot, since it does not depend on it. Compiled only under
//! `--cfg nupur_lint_probe`, by `rake dylint`, which fails unless the lints
//! report exactly the lines that end in `//~`, each by the lints named
//! after it.
#![allow(dead_code, clippy::all)]

/// A raising C call outside `raw::call`'s closure
/// (`raising_call_outside_protect`).
#[allow(unsafe_code)]
fn raising() {
    let _ = unsafe { nupur_sys::rb_ary_new_capa(0) }; //~ ffi_without_gvl_proof raising_call_outside_protect
}

/// An `unsafe fn` of `nupur_sys`, whose contract is the GVL, from a safe
/// function with no proof (`ffi_without_gvl_proof`).
#[allow(unsafe_code)]
fn sys_contract() {
    unsafe { nupur_sys::protect::forget_errinfo() } //~ ffi_without_gvl_proof
}

/// A raw `VALUE` outside the files that are the typed values
/// (`raw_ruby_outside_bridge`).
fn raw_type(_: nupur_sys::VALUE) {} //~ raw_ruby_outside_bridge

/// The GVL's token made outside the entries Ruby calls
/// (`gvl_token_outside_entry`): the maker outside `rb::method`, a
/// trampoline's helper outside a `method!` expansion, `run_init` outside an
/// `Init_` function.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod entry {
    fn token() {
        let _ = unsafe { crate::rb::Ruby::get_unchecked() }; //~ gvl_token_outside_entry
    }

    fn trampoline() -> crate::rb::Value {
        unsafe { crate::rb::method::finish(|ruby| Ok(ruby.qnil())) } //~ gvl_token_outside_entry
    }

    fn init(check: fn(&crate::rb::Ruby) -> Result<(), crate::rb::Error>) {
        unsafe { crate::rb::method::run_init(check, check) } //~ gvl_token_outside_entry
    }

    fn sys_proof() -> &'static nupur_sys::gvl::Gvl {
        unsafe { nupur_sys::gvl::Gvl::assume() } //~ ffi_without_gvl_proof gvl_token_outside_entry
    }
}

/// A handle that holds its object forgotten (`forgotten_handle`).
#[rustfmt::skip] // keeps each marker on the line reported
mod handles {
    struct P;
    impl crate::typed::Payload for P {}
    static T: crate::typed::TypedType<P> = crate::typed::TypedType::base(c"lint_probe");

    fn forgotten(v: crate::rb::Value) -> Result<(), crate::rb::Error> {
        let handle = T.check(v)?;
        core::mem::forget(handle); //~ forgotten_handle
        Ok(())
    }

    fn manually(v: crate::rb::Value) -> Result<(), crate::rb::Error> {
        let _kept = core::mem::ManuallyDrop::new(T.check(v)?); //~ forgotten_handle
        Ok(())
    }
}

/// A C global of Ruby's - a raw `VALUE` with no type spelled - outside them
/// too.
#[allow(unsafe_code)]
fn raw_global() -> bool {
    unsafe { nupur_sys::rb_cObject != 0 } //~ raw_ruby_outside_bridge
}

/// Ruby's C API from a function holding no proof of the GVL
/// (`ffi_without_gvl_proof`) - and the same with one, which passes.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod gvl_proof {
    fn without_proof() -> bool {
        unsafe { nupur_sys::ruby_stack_check() != 0 } //~ ffi_without_gvl_proof
    }

    fn with_a_token(_: &crate::rb::Ruby) -> bool {
        unsafe { nupur_sys::ruby_stack_check() != 0 }
    }

    fn with_a_value(_: crate::rb::Value) -> bool {
        unsafe { nupur_sys::ruby_stack_check() != 0 }
    }

    fn in_a_closure() -> bool {
        (|| unsafe { nupur_sys::ruby_stack_check() != 0 })() //~ ffi_without_gvl_proof
    }
}

/// A C integer at the Ruby boundary outside `nupur::int`
/// (`c_integer_outside_int`): an integer `as` cast - which the lint sees
/// even where `c_long` is `i64` itself - and `c_long` named; a `TryFrom`
/// passes.
#[rustfmt::skip] // keeps each marker on the line reported
mod c_integer {
    fn cast(n: usize) -> i64 {
        n as i64 //~ c_integer_outside_int
    }

    fn named(n: i64) -> Option<core::ffi::c_long> { //~ c_integer_outside_int
        n.try_into().ok()
    }

    fn converted(n: usize) -> Option<i64> {
        i64::try_from(n).ok()
    }
}

/// Which C functions raise (`raising_call_outside_protect`): one nobody has
/// classified is taken to raise - `rb_undef_alloc_func`, which a list of the
/// raising ones did not name - and one the table says raises nothing passes.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod raising {
    fn unclassified(_: &crate::rb::Ruby) {
        unsafe { nupur_sys::rb_undef_alloc_func(nupur_sys::rb_cObject) } //~ raising_call_outside_protect raw_ruby_outside_bridge
    }

    fn classified(_: &crate::rb::Ruby) -> bool {
        unsafe { nupur_sys::rb_block_given_p() != 0 }
    }
}

/// What `raw` holds (`unraising_call_in_raw`), in a stand-in for it: a call
/// of Ruby's C outside `call`, and a `call` that protects nothing, are
/// reported; a protected call that can raise is what `raw` is.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod raw {
    fn call<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    fn a_read_in_raw(_: &crate::rb::Ruby) -> bool {
        unsafe { nupur_sys::rb_block_given_p() != 0 } //~ unraising_call_in_raw
    }

    fn protecting_nothing(_: &crate::rb::Ruby) -> bool {
        call(|| unsafe { nupur_sys::rb_block_given_p() != 0 }) //~ unraising_call_in_raw
    }

    fn protecting_a_raise(_: &crate::rb::Ruby) {
        let _ = call(|| unsafe { nupur_sys::rb_ary_new_capa(0) });
    }
}
