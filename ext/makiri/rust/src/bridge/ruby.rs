//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are the places the extension still calls such a function,
//! and each turns the raise into a [`magnus::Error`] instead: the caller hands
//! it back as `Err`, and magnus raises only after the Rust frames have returned
//! normally. [`raise`] is the one exit for an entry point Ruby calls with the C
//! convention, which has no `Result` to return.

#![allow(unsafe_code)]

use core::ffi::{c_long, c_void};

use magnus::rb_sys::{protect, AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, RString, Ruby, Value};
use rb_sys::{rb_data_type_t, VALUE};

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
pub struct DataType(rb_sys::rb_data_type_t);

// SAFETY: the contents are set once at compile time and never mutated. Ruby
// reads them from whichever thread holds the GVL.
unsafe impl Sync for DataType {}

impl DataType {
    /// `parent` is null for a base type.
    pub const fn new(
        name: *const core::ffi::c_char,
        parent: *const rb_sys::rb_data_type_t,
        dmark: rb_sys::RUBY_DATA_FUNC,
        dfree: rb_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> rb_sys::size_t>,
    ) -> DataType {
        DataType(rb_sys::rb_data_type_t {
            wrap_struct_name: name,
            function: rb_sys::rb_data_type_struct__bindgen_ty_1 {
                dmark,
                dfree,
                dsize,
                dcompact: None,
                reserved: [core::ptr::null_mut(); 1],
            },
            parent,
            data: core::ptr::null_mut(),
            flags: rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE,
        })
    }

    /// The raw pointer the Ruby API wants.
    #[inline]
    pub const fn as_ptr(&self) -> *const rb_sys::rb_data_type_t {
        self as *const DataType as *const rb_sys::rb_data_type_t
    }
}

/// `v` as a String, coerced the way `rb_String` does (`to_str`, else `to_s`).
///
/// A String passes straight through, so the common case is one type check.
/// Anything else runs its conversion under `protect`: a `to_s` that raises
/// comes back as `Err` rather than unwinding through the caller.
pub fn string_of(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        return Ok(s);
    }
    // SAFETY: `v` is a live value; `protect` turns a raising `to_s` into `Err`.
    let s = protect(|| unsafe { rb_sys::rb_String(v.as_raw()) })?;
    // SAFETY: `rb_String` returned a live String.
    Ok(RString::from_value(unsafe { Value::from_raw(s) }).expect("rb_String returns a String"))
}

/// `rb_check_frozen` returning its FrozenError rather than raising it.
///
/// The error is Ruby's own - the message naming the receiver and `#receiver`
/// set - because the check still runs, under `protect`. An unfrozen value, every
/// mutator's normal case, never enters `protect`.
pub fn check_frozen(v: Value) -> Result<(), Error> {
    if !magnus::value::ReprValue::is_frozen(v) {
        return Ok(());
    }
    // SAFETY: `v` is a live value; `protect` turns the raise into `Err`.
    protect(|| unsafe {
        rb_sys::rb_check_frozen(v.as_raw());
        rb_sys::Qnil as VALUE
    })
    .map(|_| ())
}

/// `rb_respond_to` returning a raise from the object's own `respond_to?` (or the
/// `respond_to_missing?` behind it) as `Err`, rather than unwinding through the
/// caller.
pub fn respond_to(v: Value, method: rb_sys::ID) -> Result<bool, Error> {
    // SAFETY: `v` is a live value; `protect` turns the raise into `Err`.
    let answer = protect(|| unsafe {
        if rb_sys::rb_respond_to(v.as_raw(), method) != 0 {
            rb_sys::Qtrue as VALUE
        } else {
            rb_sys::Qfalse as VALUE
        }
    })?;
    Ok(answer == rb_sys::Qtrue as VALUE)
}

/// `rb_range_beg_len` over a collection of `count` elements, with the C's
/// `err = 0`: a start outside the collection is `None` rather than a raise.
///
/// A bound too large for a `long` still raises (`RangeError`), and that raise
/// comes back as `Err` - the caller holds the collection's nodes in a `Vec` a
/// longjmp would leak.
pub fn range_beg_len(range: Value, count: c_long) -> Result<Option<(c_long, c_long)>, Error> {
    let mut beg: c_long = 0;
    let mut len: c_long = 0;
    // SAFETY: `range` is a live Range; `protect` turns the raise into `Err`.
    let answer = protect(|| unsafe {
        rb_sys::rb_range_beg_len(range.as_raw(), &mut beg, &mut len, count, 0)
    })?;
    Ok((answer == rb_sys::Qtrue as VALUE).then_some((beg, len)))
}

/// The data pointer of a TypedData object of type `ty` (or a type deriving
/// from it), or the `TypeError` Ruby's own check raises.
///
/// The type is tested first, so a well-typed object - every call on the normal
/// path - never enters `protect`. Only a mismatch runs `rb_check_typeddata`
/// under it, which is what keeps the error message Ruby's own, word for word,
/// on every supported Ruby.
pub fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: `v` is a live value and `ty` a registered data type. The check
    // runs unprotected only once the type is known to match, so it cannot
    // raise there; a mismatch runs it under `protect`.
    unsafe {
        if rb_sys::rb_typeddata_is_kind_of(v, ty) != 0 {
            return Ok(rb_sys::rb_check_typeddata(v, ty));
        }
        match protect(|| rb_sys::rb_check_typeddata(v, ty) as VALUE) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(Error::new(
                magnus::Ruby::get_unchecked().exception_type_error(),
                "wrong argument type",
            )),
        }
    }
}

/// The data pointer of a TypedData object whose type the caller has already
/// established - a receiver magnus converted, or the Document a checked node
/// holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// magnus turns it into a fatal error, where a raise would longjmp past them.
pub fn typed_data_known(v: Value, ty: &'static DataType) -> *mut c_void {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: as in `typed_data`; the assert makes the check that follows one
    // that cannot raise.
    unsafe {
        assert!(
            rb_sys::rb_typeddata_is_kind_of(v, ty) != 0,
            "a VALUE of an established type had a different one"
        );
        rb_sys::rb_check_typeddata(v, ty)
    }
}

/// The wrapped Rust value behind a TypedData object, without magnus's
/// `rb_protect`.
///
/// `<&T>::try_convert` - and so every magnus method with a wrapped receiver -
/// runs `rb_check_typeddata` inside `rb_protect`, which is a `setjmp` per call.
/// That is the right default when a Rust caller wants a `Result`, but it is not
/// free: on the per-node path it measured about a quarter of the throughput of
/// the C it replaced (`Node#css` over 2000 nodes, `notes/node_set_ab.rb`).
///
/// # Safety
/// Raises (longjmps) when `v` is not a `T`, so no Rust destructor may be live.
/// Only for a VALUE the caller built as a `T` itself. The returned lifetime is
/// unconstrained; the caller must keep `v` rooted.
pub unsafe fn typed_data_unprotected<'a, T: magnus::TypedData>(v: VALUE) -> &'a T {
    /* magnus::DataType is #[repr(transparent)] over rb_data_type_t, so this
     * cast is what the repr promises; the accessor for it is crate-private. */
    let dt = T::data_type() as *const magnus::typed_data::DataType as *const rb_data_type_t;
    &*(rb_sys::rb_check_typeddata(v, dt) as *const T)
}

/* ------------------------------------------------------------------ *
 * Value-level helpers                                                *
 * ------------------------------------------------------------------ */

/* The singletons and constructors the glue would otherwise spell as rb_sys
 * constants and raw calls.
 *
 * `nil` and `boolean` are immortal singletons and cannot raise.
 *
 * The constructors DO allocate through Ruby - `integer` for a value that is
 * not a fixnum, `float` and `array_new` always - so an out-of-memory there
 * RAISES `NoMemoryError`, which unwinds with `longjmp` and not as an `Err`.
 * This is the same OOM the raw `rb_float_new`/`rb_int2inum`/`rb_ary_new` they
 * replace could raise, so no call site changed; it is also why they are not
 * protected yet. A caller that holds a live Rust destructor across one must
 * run the whole conversion under `protect` (as `glue::xpath::value_to_ruby`
 * does), and turning them into `Result`-returning helpers is part of moving
 * the protected calls behind the bridge. */

/// `nil`, as a `Value`.
#[inline]
pub fn nil() -> Value {
    // SAFETY: every caller is a Ruby method, entered with the GVL.
    unsafe { Ruby::get_unchecked() }.qnil().as_value()
}

/// `true`/`false`, as a `Value`.
#[inline]
pub fn boolean(b: bool) -> Value {
    // SAFETY: as `nil`.
    let ruby = unsafe { Ruby::get_unchecked() };
    if b {
        ruby.qtrue().as_value()
    } else {
        ruby.qfalse().as_value()
    }
}

/// A fresh Float.
#[inline]
pub fn float(n: f64) -> Value {
    // SAFETY: every caller is a Ruby method, entered with the GVL.
    unsafe { Ruby::get_unchecked() }.float_from_f64(n).as_value()
}

/// An Integer from an `i64`.
#[inline]
pub fn integer(n: i64) -> Value {
    // SAFETY: as `float`.
    unsafe { Ruby::get_unchecked() }.integer_from_i64(n).as_value()
}

/// A fresh empty Array.
#[inline]
pub fn array_new() -> Value {
    // SAFETY: as `float`.
    unsafe { Ruby::get_unchecked() }.ary_new().as_value()
}

/// The frame's current receiver.
///
/// magnus hands a method a `&T`, not the object; the registrars that return
/// `self` recover it from the frame. `Err` only when Ruby has no current
/// receiver, which a method invocation always has.
#[inline]
pub fn current_receiver() -> Result<Value, Error> {
    // SAFETY: as `float`.
    unsafe { Ruby::get_unchecked() }.current_receiver::<Value>()
}

/// VALUE identity, for the several sites that compare two references.
#[inline]
pub fn same_value(a: Value, b: Value) -> bool {
    a.as_raw() == b.as_raw()
}

/// Run `f` under `rb_protect`, so a raise inside it comes back as `Err`.
///
/// `f` is the raw-building body; the VALUE it returns on success is handed
/// back as a `Value`. A raise is a `longjmp`, which skips the Rust destructors
/// in every frame it crosses - so `f` must own nothing a raise could leak, or
/// the caller must accept the leak (the CSS and XPath result builders snapshot
/// their `Vec` first and free it on the `Err` path).
#[inline]
pub fn protect_value<F>(f: F) -> Result<Value, Error>
where
    F: FnOnce() -> VALUE,
{
    // SAFETY: `protect` establishes the setjmp frame the raise unwinds to, and
    // the VALUE it hands back on success is live.
    protect(f).map(|raw| unsafe { Value::from_raw(raw) })
}

/// Allocate a zeroed `T`, fill it with `init`, wrap it as a `klass` object of
/// data type `ty`, and only then let `store` write the VALUEs it holds.
///
/// The order is the point. The wrap allocates, so it is a GC point, and a VALUE
/// already sitting in this malloc'd struct is seen by no mark there: a GC can
/// free it, or compaction move it out from under the stored copy. Zeroed, a
/// VALUE field reads as `false` to the mark until `store` sets it; and the
/// VALUEs `store` writes are still on the caller's stack across the wrap,
/// where the conservative scan pins them.
///
/// `ruby_xcalloc` raises `NoMemoryError` on OOM; nothing is owned at that
/// point, which is the fallible-allocation line for glue-side buffers.
///
/// # Safety
/// Under the GVL. `T` must be valid when zeroed, and `ty` must free it with
/// `ruby_xfree`.
pub unsafe fn wrap_zeroed<T>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    init: impl FnOnce(&mut T),
    store: impl FnOnce(&mut T),
) -> VALUE {
    let data = rb_sys::ruby_xcalloc(1, core::mem::size_of::<T>() as rb_sys::size_t) as *mut T;
    init(&mut *data);
    let obj = rb_sys::rb_data_typed_object_wrap(klass, data as *mut c_void, ty);
    store(&mut *data);
    obj
}
