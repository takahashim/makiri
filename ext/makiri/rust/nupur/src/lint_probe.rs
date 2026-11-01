//! What the extension's lints (`lints/makiri_lints`) must report in the
//! binding layer, the cases only code that names `nupur_sys` can write - the
//! extension cannot, since it does not depend on it. A call of rb-sys's own
//! C cannot be written here at all (`nupur_sys` re-exports no function): its
//! cases are `nupur_sys`'s probe's. Compiled only under
//! `--cfg nupur_lint_probe`, by `rake dylint`, which fails unless the lints
//! report exactly the lines that end in `//~`, each by the lints named
//! after it.
#![allow(dead_code, clippy::all)]

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

/// What `raw` holds (`unraising_call_in_raw`), in a stand-in for it: a
/// `call` whose closure calls nothing that can raise is reported - a read
/// there needs no protection.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod raw {
    fn call<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    fn protecting_nothing(ruby: &crate::rb::Ruby) -> bool {
        call(|| unsafe { nupur_sys::read::rb_block_given_p(ruby.gvl()) != 0 }) //~ unraising_call_in_raw
    }
}
