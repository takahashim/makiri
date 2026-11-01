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

/// `nupur_sys`'s value types on the heap (`ruby_value_on_heap`): they hold
/// a `VALUE` and implement no trait of the layer's, so the lint names them.
struct SysKept {
    values: Vec<nupur_sys::value::Value>, //~ ruby_value_on_heap
    roots: Vec<nupur_sys::value::Root>,   //~ ruby_value_on_heap
}

/// The GVL's token made outside the entries Ruby calls
/// (`gvl_token_outside_entry`): the maker outside `rb::method`, a
/// trampoline's helper outside a `method!` expansion, `run_init` outside an
/// `Init_` function.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod entry {

    fn trampoline() -> crate::rb::Value {
        unsafe { crate::rb::method::finish(|ruby| Ok(ruby.qnil())) } //~ gvl_token_outside_entry
    }

    fn init(check: fn(&crate::rb::Ruby) -> Result<(), crate::rb::Error>) {
        unsafe { crate::rb::method::run_init(check, check) } //~ gvl_token_outside_entry
    }

    fn sys_token() -> nupur_sys::value::Ruby {
        unsafe { nupur_sys::value::Ruby::assume_gvl() } //~ ffi_without_gvl_proof gvl_token_outside_entry
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

/// A C integer at the Ruby boundary outside `nupur_sys::int`
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

/// What `raw` holds (`unraising_call_in_raw`), in a stand-in for it: a read
/// that raises nothing, outside `call` or as all a `call` protects, is
/// reported.
#[allow(unsafe_code)]
#[rustfmt::skip] // keeps each marker on the line reported
mod raw {
    fn call<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    /* The protected form, whose closure is lent the token: never run here,
     * only checked. */
    fn call_protected<T>(_: impl FnOnce(&nupur_sys::protect::Protected) -> T) -> Option<T> {
        None
    }

    fn a_read_in_raw(ruby: &crate::rb::Ruby) -> bool {
        unsafe { nupur_sys::non_raising::rb_block_given_p(ruby.sys()) != 0 } //~ unraising_call_in_raw
    }

    fn protecting_nothing(ruby: &crate::rb::Ruby) -> bool {
        call(|| unsafe { nupur_sys::non_raising::rb_block_given_p(ruby.sys()) != 0 }) //~ unraising_call_in_raw
    }

    /* A call of a raising wrapper is what a protected call is for: passes. */
    fn protecting_a_raise(_: &crate::rb::Ruby) {
        let _ = call_protected(|inside| unsafe { nupur_sys::raising::rb_hash_new(inside) });
    }
}
