//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are the places the extension still calls such a function,
//! and each turns the raise into a [`magnus::Error`] instead: the caller hands
//! it back as `Err`, and magnus raises only after the Rust frames have returned
//! normally. [`raise`] is the one exit for an entry point Ruby calls with the C
//! convention, which has no `Result` to return.

use core::ffi::{c_int, c_void};

use magnus::error::ErrorType;
use magnus::rb_sys::{protect, AsRawValue, FromRawValue};
use magnus::{Error, RString, Value};
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

/// Raise `e` from an entry point Ruby calls with the C convention.
///
/// Only for the frame Ruby itself called: it unwinds with `longjmp`, so nothing
/// between Ruby and this call may own a resource. The error is turned into
/// Ruby objects and dropped before the jump, so it does not leak either.
///
/// # Safety
/// Under the GVL, from a frame whose callers own nothing that needs dropping.
pub unsafe fn raise(e: Error) -> ! {
    let jump = match e.error_type() {
        ErrorType::Exception(x) => Err(x.as_raw()),
        ErrorType::Error(class, msg) => {
            let s = rb_sys::rb_utf8_str_new(msg.as_ptr() as *const _, msg.len() as _);
            Err(rb_sys::rb_exc_new_str(class.as_raw(), s))
        }
        ErrorType::Jump(tag) => Ok(*tag as c_int),
    };
    drop(e);
    match jump {
        Err(exc) => rb_sys::rb_exc_raise(exc),
        Ok(tag) => rb_sys::rb_jump_tag(tag),
    }
}
