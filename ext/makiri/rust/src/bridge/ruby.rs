//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are the places the extension still calls such a function,
//! and each turns the raise into a [`magnus::Error`] instead: the caller hands
//! it back as `Err`, and magnus raises only after the Rust frames have returned
//! normally.

#![allow(unsafe_code)]

use core::ffi::{c_int, c_long};

use magnus::rb_sys::{protect as magnus_protect, AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, RString, Ruby, Value};

/// The raw handle types, re-exported so a higher layer can name a `VALUE` or a
/// method `ID` without reaching into `rb_sys` itself.
pub use rb_sys::{ID, VALUE};

/* The two conveniences every bridge and glue module shares. */

/// `Makiri::Error` - the one definition; the modules that each kept a private
/// copy of this now import it.
pub fn error_class() -> magnus::ExceptionClass {
    crate::init::EXC_ERROR.exception()
}

/// A `Makiri::Error` carrying `msg` - the one way the extension raises its own
/// error class, so a caller never spells the class (or finds a stale copy of
/// the lookup) itself.
pub fn makiri_error(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(error_class(), msg)
}

/// Is `v` an instance of the class in `klass`? `false` before `Init_makiri`,
/// when no class of ours exists for it to be an instance of.
#[inline]
pub fn is_kind_of(v: Value, klass: &crate::init::RbConst<magnus::RClass>) -> bool {
    klass.get().is_some_and(|c| v.is_kind_of(c))
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
    // SAFETY: `rb_String` returned a live value.
    RString::from_value(unsafe { Value::from_raw(s) })
        .ok_or_else(|| type_error("String() returned a non-String"))
}

/// `v.to_s`, as a String: Ruby's own `to_s`, so a subclass's or other object's
/// override is honoured, as Nokogiri's `to_s` coercions are. A `to_s` that
/// returns anything but a String is a `TypeError`, not a value later read as
/// one.
///
/// An instance whose class is exactly `String` is returned as it is, without a
/// call: paying a method dispatch on every string is not worth it. That skips
/// a `to_s` defined on the instance's own singleton class - `rb_obj_class` does
/// not see singletons - which is why the fast path tests the class, not
/// `respond_to?`.
pub fn to_s(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        if v.class().as_raw() == gvl_ruby().class_string().as_raw() {
            return Ok(s);
        }
    }
    let r: Value = v.funcall("to_s", ())?;
    RString::from_value(r).ok_or_else(|| type_error("to_s returned a non-String"))
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

/* ------------------------------------------------------------------ *
 * Value-level helpers                                                *
 * ------------------------------------------------------------------ */

/* The singletons and constructors the glue would otherwise spell as rb_sys
 * constants and raw calls.
 *
 * `nil` and `boolean` are immortal singletons and cannot raise.
 *
 * The constructors DO allocate through Ruby - `float` and `array_new` - so an out-of-memory there RAISES
 * `NoMemoryError`, which unwinds with `longjmp` and not as an `Err`. This is
 * the same OOM the raw `rb_float_new`/`rb_ary_new` they replace could raise.
 * A caller that holds a live Rust destructor across one must run the whole
 * conversion under `protect`, as `bridge::xpath::value_to_ruby` does. */

/// A `Value` from a raw handle the caller already holds - a stored field, or a
/// producer that still returns `VALUE`. The bridge is the one place that turns
/// a raw handle back into a `Value`.
///
/// # Safety
/// `raw` must be a live Ruby value of the current process (as every `VALUE` a
/// wrapper stores is), and the caller must keep it rooted.
#[inline]
pub unsafe fn value(raw: VALUE) -> Value {
    // SAFETY: the caller's contract.
    unsafe { Value::from_raw(raw) }
}

/// The Ruby handle the helpers below build their values with, unchecked.
///
/// Every caller of this module's value and error helpers is a Ruby method - or
/// `Init_makiri` - so it runs on a Ruby thread holding the GVL, which is the
/// one precondition `Ruby::get_unchecked` has. Stated here once, rather than
/// at each helper; a caller off a Ruby thread (the GVL-released parse) takes
/// no Ruby value at all, which `bridge::gvl::without_gvl`'s `Send` bound keeps.
/// `init::RbConst`'s readers are helpers of the same kind.
#[inline]
pub(crate) fn gvl_ruby() -> Ruby {
    // SAFETY: see above - on a Ruby thread, with the GVL.
    unsafe { Ruby::get_unchecked() }
}

/// `nil`, as a `Value`.
#[inline]
pub fn nil() -> Value {
    gvl_ruby().qnil().as_value()
}

/// `true`/`false`, as a `Value`.
#[inline]
pub fn boolean(b: bool) -> Value {
    let ruby = gvl_ruby();
    if b {
        ruby.qtrue().as_value()
    } else {
        ruby.qfalse().as_value()
    }
}

/// A fresh Float.
#[inline]
pub fn float(n: f64) -> Value {
    gvl_ruby().float_from_f64(n).as_value()
}

/// A fresh empty Array.
#[inline]
pub fn array_new() -> Value {
    gvl_ruby().ary_new().as_value()
}

/// The frame's current receiver.
///
/// magnus hands a method a `&T`, not the object; the registrars that return
/// `self` recover it from the frame. `Err` only when Ruby has no current
/// receiver, which a method invocation always has.
#[inline]
pub fn current_receiver() -> Result<Value, Error> {
    gvl_ruby().current_receiver::<Value>()
}

/// A `TypeError` with `msg`.
pub fn type_error(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(gvl_ruby().exception_type_error(), msg)
}

/// An `ArgumentError` with `msg`.
pub fn arg_error(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(gvl_ruby().exception_arg_error(), msg)
}

/// VALUE identity, for the several sites that compare two references.
#[inline]
pub fn same_value(a: Value, b: Value) -> bool {
    a.as_raw() == b.as_raw()
}

/// A method `ID`, interned from a UTF-8 name.
///
/// UTF-8 explicitly, not `rb_intern`: that one tags the name US-ASCII, so a
/// non-ASCII name (`é`) became a different symbol from the UTF-8 method Ruby
/// defined under it, and the method was never found.
#[inline]
pub fn intern(name: &str) -> ID {
    // SAFETY: `name` is `name.len()` valid UTF-8 bytes, read for the call;
    // interning valid UTF-8 does not raise.
    unsafe {
        rb_sys::rb_intern3(
            name.as_ptr() as *const core::ffi::c_char,
            name.len() as core::ffi::c_long,
            rb_sys::rb_utf8_encoding(),
        )
    }
}

/// `rb_funcallv`: call `method` on `recv`. Can raise, so the caller runs it
/// under [`protect_value`].
///
/// # Safety
/// Under the GVL, and no Rust destructor may be live when it raises.
#[inline]
pub unsafe fn funcallv(recv: VALUE, method: ID, args: &[VALUE]) -> VALUE {
    // SAFETY: the caller's contract; `args` is a live slice.
    unsafe { rb_sys::rb_funcallv(recv, method, args.len() as c_int, args.as_ptr()) }
}

/// `true`/`false` for Ruby's two boolean singletons, and `None` for anything
/// else (which a caller treats as neither).
#[inline]
pub fn bool_value(v: VALUE) -> Option<bool> {
    let ruby = gvl_ruby();
    if v == ruby.qtrue().as_raw() {
        Some(true)
    } else if v == ruby.qfalse().as_raw() {
        Some(false)
    } else {
        None
    }
}

/// A Ruby entry point that faces untrusted input: a panic inside becomes
/// `Makiri::InternalError` rather than Ruby's `fatal`.
///
/// Both are internal errors and neither is a `StandardError`, so a bare
/// `rescue => e` keeps passing them through - a broken invariant is not a bad
/// selector. The difference is that a `fatal` cannot be rescued AT ALL in the
/// frame that raised it, so a host with no thread boundary around the call
/// loses the process; `InternalError` it can catch and turn into a 500.
///
/// Every method the glue registers starts with this - `rake unsafe:boundaries`
/// fails on one that does not, bar its short `ENTRY_EXEMPT` list - so a panic
/// under any of them is this exception. Elsewhere it is still `fatal`.
#[inline]
pub fn entry<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    match std::panic::catch_unwind(core::panic::AssertUnwindSafe(f)) {
        Ok(out) => out,
        Err(payload) => Err(Error::new(
            crate::init::EXC_INTERNAL_ERROR.exception(),
            crate::caught::message(payload.as_ref()).to_owned(),
        )),
    }
}

/// magnus's `protect`, with a Rust panic caught instead of sent into C.
///
/// magnus runs the closure inside an `extern "C"` trampoline that `rb_protect`
/// calls, so a panic in it would hit that boundary and abort the process. This
/// catches it, lets `rb_protect` return normally - Ruby sees an ordinary `nil`
/// answer, and every `ensure` it owns runs - and re-raises from HERE, where
/// only Rust frames are left. See [`crate::caught`].
///
/// Every `protect` in the crate goes through this one, which is the point: the
/// hazard is a property of `rb_protect`, not of any particular closure.
#[inline]
pub fn protect<F>(f: F) -> Result<VALUE, Error>
where
    F: FnOnce() -> VALUE,
{
    let mut latch = crate::caught::PanicLatch::new();
    let out = magnus_protect(|| latch.guard(rb_sys::Qnil as VALUE, f));
    latch.resume();
    out
}

#[inline]
/// Run `f` under `rb_protect`, so a raise inside it comes back as `Err`.
///
/// `f` is the raw-building body; the VALUE it returns on success is handed
/// back as a `Value`. A raise is a `longjmp`, which skips the Rust destructors
/// in every frame it crosses - so `f` must own nothing a raise could leak, or
/// the caller must accept the leak (the CSS and XPath result builders snapshot
/// their `Vec` first and free it on the `Err` path).
pub fn protect_value<F>(f: F) -> Result<Value, Error>
where
    F: FnOnce() -> VALUE,
{
    // SAFETY: `protect` establishes the setjmp frame the raise unwinds to, and
    // the VALUE it hands back on success is live.
    protect(f).map(|raw| unsafe { Value::from_raw(raw) })
}

/// Tell the GC about `delta` bytes held outside its heap (negative: given
/// back). Memory Ruby does not allocate is otherwise invisible to it, and never
/// triggers a collection.
pub fn report_external_bytes(delta: isize) {
    // SAFETY: a plain counter adjustment, with the GVL held - from a method
    // or from Ruby's free hook, where it only subtracts.
    unsafe { rb_sys::rb_gc_adjust_memory_usage(delta as rb_sys::ssize_t) };
}

/* ---- exception messages ---- */

unsafe extern "C" fn exception_message_thunk(exc: VALUE) -> VALUE {
    rb_sys::rb_obj_as_string(rb_sys::rb_funcall(
        exc,
        rb_sys::rb_intern(c"message".as_ptr()),
        0,
    ))
}

/// The longest message kept: this runs on error paths to word another error,
/// and a handler's exception message is untrusted input.
const EXCEPTION_MESSAGE_MAX: usize = 255;

/// `exc`'s message, for wording another error with it: up to the first NUL,
/// at most [`EXCEPTION_MESSAGE_MAX`] bytes, invalid UTF-8 replaced. "error" if
/// asking for the message raises or answers with a non-String - this runs on
/// error paths, so it must not raise itself.
pub fn exception_message(exc: VALUE) -> String {
    let mut state: c_int = 0;
    // SAFETY: the thunk is only C calls, run under rb_protect; a raise inside
    // comes back as a nonzero state, which is cleared.
    let msg = unsafe { rb_sys::rb_protect(Some(exception_message_thunk), exc, &mut state) };
    if state != 0 {
        // SAFETY: clearing the error the protected call left, under the GVL.
        unsafe { rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE) };
        return "error".to_owned();
    }
    // SAFETY: `msg` is the live VALUE the call returned.
    let Some(r) = magnus::RString::from_value(unsafe { Value::from_raw(msg) }) else {
        return "error".to_owned();
    };
    // SAFETY: copied out before anything below can run Ruby.
    let bytes = unsafe { r.as_slice() };
    let n = bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(bytes.len())
        .min(EXCEPTION_MESSAGE_MAX);
    String::from_utf8_lossy(&bytes[..n]).into_owned()
}

/// [`exception_message`] for a magnus error: the Ruby exception's message, or
/// "error" for one that carries none.
pub fn error_message(e: &Error) -> String {
    match e.error_type() {
        magnus::error::ErrorType::Exception(x) => exception_message(x.as_raw()),
        _ => "error".to_owned(),
    }
}
