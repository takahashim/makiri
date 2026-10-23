//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are built on `bridge::raw`'s wrappers, each of which turns
//! the raise into a [`magnus::Error`]: the caller hands it back as `Err`, and
//! magnus raises only after the Rust frames have returned normally.

#![allow(unsafe_code)]

use core::ffi::c_long;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, RString, Ruby, Value};

/// The raw handle types, re-exported so a higher layer can name a `VALUE` or a
/// method `ID` without reaching into `rb_sys` itself.
pub use rb_sys::{ID, VALUE};

/* The two conveniences every bridge and glue module shares. */

/// The `MAJOR.MINOR` of the Ruby whose headers this extension was compiled
/// against - its ABI, since a teeny release keeps it. Published as
/// `Makiri::NATIVE_RUBY_API_VERSION`.
pub fn ruby_api_version() -> String {
    format!(
        "{}.{}",
        rb_sys::RUBY_API_VERSION_MAJOR,
        rb_sys::RUBY_API_VERSION_MINOR
    )
}

/// `Err(LoadError)` when the running Ruby's API `MAJOR.MINOR` is not the one
/// this extension was compiled for.
///
/// The FIRST thing `Init_makiri` does. A release Ruby does not check
/// `ruby_abi_version` itself, and under macOS's `-undefined dynamic_lookup`
/// a binary built for another Ruby loads and then misreads the interpreter's
/// structs - defining one class could already corrupt it, so the check
/// cannot wait for `lib/makiri.rb`. It reads the running Ruby's exported
/// `ruby_api_version` (three ints, whose layout no Ruby changes) and nothing
/// else; only a mismatch goes on to build the exception.
pub fn check_running_ruby_api(ruby: &Ruby) -> Result<(), Error> {
    // SAFETY: an immutable `const int[3]` libruby exports, read by value.
    let running = unsafe { (rb_sys::ruby_api_version[0], rb_sys::ruby_api_version[1]) };
    let built = (
        rb_sys::RUBY_API_VERSION_MAJOR as core::ffi::c_int,
        rb_sys::RUBY_API_VERSION_MINOR as core::ffi::c_int,
    );
    if running == built {
        return Ok(());
    }
    Err(Error::new(
        ruby.exception_load_error(),
        format!(
            "makiri's native extension was built for Ruby {}.{}, but this is Ruby {}.{}; \
             rebuild it (`bundle exec rake clean compile` in a checkout, or reinstall the gem)",
            built.0, built.1, running.0, running.1
        ),
    ))
}

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

/// A `Makiri::InternalError` carrying `msg`: a broken invariant of Makiri
/// itself, which a bare `rescue` (StandardError) passes through - see
/// [`entry`].
pub fn internal_error(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(crate::init::EXC_INTERNAL_ERROR.exception(), msg)
}

/// Is `v` an instance of the class in `klass`? `false` before `Init_makiri`,
/// when no class of ours exists for it to be an instance of.
#[inline]
pub fn is_kind_of(v: Value, klass: &crate::init::RbConst<magnus::RClass>) -> bool {
    klass.get().is_some_and(|c| v.is_kind_of(c))
}

/// `v`, which must be a String (a subclass's instance included).
///
/// There is no conversion: a `to_str` or `to_s` is the caller's Ruby code, and
/// Makiri runs none, so anything else is a `TypeError`, worded as Ruby's own
/// type check words it.
pub fn string_arg(v: Value) -> Result<RString, Error> {
    RString::from_value(v).ok_or_else(|| wrong_type(v, "String"))
}

/// `v` as a String for a NAME (an attribute or element name, a prefix, a
/// selector, an expression): a String, or a Symbol's name. Nothing else
/// converts, as [`string_arg`].
pub fn name_arg(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        return Ok(s);
    }
    if magnus::Symbol::from_value(v).is_none() {
        return Err(wrong_type(v, "String or Symbol"));
    }
    // SAFETY: `v` is a live Symbol.
    let s = unsafe { super::raw::sym2str(v.as_raw()) }?;
    // SAFETY: `rb_sym2str` returned a live String.
    RString::from_value(unsafe { Value::from_raw(s) })
        .ok_or_else(|| type_error("a Symbol without a name"))
}

/// `v` for an error message, written without calling it: a Symbol as
/// `:name`, a String quoted, anything else by its class (`#inspect` would be
/// the caller's Ruby).
pub fn describe(v: Value) -> String {
    if magnus::Symbol::from_value(v).is_some() {
        if let Ok(s) = name_arg(v) {
            // SAFETY: copied out at once.
            return format!(":{}", String::from_utf8_lossy(unsafe { s.as_slice() }));
        }
    }
    if let Some(s) = RString::from_value(v) {
        // SAFETY: copied out at once.
        return format!("{:?}", String::from_utf8_lossy(unsafe { s.as_slice() }));
    }
    if v.is_nil() {
        return "nil".to_owned();
    }
    // SAFETY: a live value; its class name is copied at once.
    format!("an instance of {}", unsafe { v.classname() })
}

/// Ruby's `TypeError` for a value of the wrong class, worded as Ruby's own
/// type check words it, with the way out: "wrong argument type Integer
/// (expected String); call #to_s to convert it". Makiri converts nothing
/// itself (a `to_s` is the caller's Ruby), so the caller does. The class name
/// is read without a call.
fn wrong_type(v: Value, expected: &str) -> Error {
    // SAFETY: `v` is a live value, and its class name is copied before anything
    // else runs.
    let class = unsafe { v.classname() }.into_owned();
    type_error(format!(
        "wrong argument type {class} (expected {expected}); call #to_s to convert it"
    ))
}

/// `rb_check_frozen` returning its FrozenError rather than raising it.
///
/// The error is Ruby's own - the message naming the receiver and `#receiver`
/// set - because the check still runs, protected. An unfrozen value, every
/// mutator's normal case, makes no call.
pub fn check_frozen(v: Value) -> Result<(), Error> {
    if !magnus::value::ReprValue::is_frozen(v) {
        return Ok(());
    }
    // SAFETY: `v` is a live value.
    unsafe { super::raw::check_frozen(v.as_raw()) }
}

/// `rb_range_beg_len` over a collection of `count` elements, with the C's
/// `err = 0`: a start outside the collection is `None` rather than a raise.
///
/// A bound too large for a `long` is `Err` (`RangeError`).
pub fn range_beg_len(range: Value, count: c_long) -> Result<Option<(c_long, c_long)>, Error> {
    let mut beg: c_long = 0;
    let mut len: c_long = 0;
    // SAFETY: `range` is a live Range, and `beg`/`len` outlive the call.
    let answer = unsafe { super::raw::range_beg_len(range.as_raw(), &mut beg, &mut len, count) }?;
    Ok((answer == rb_sys::Qtrue as VALUE).then_some((beg, len)))
}

/* ------------------------------------------------------------------ *
 * Value-level helpers                                                *
 * ------------------------------------------------------------------ */

/* The singletons the glue would otherwise spell as rb_sys constants. `nil`
 * and `boolean` are immortal and cannot raise; what allocates is
 * `bridge::build`'s. */

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
        Err(payload) => Err(internal_error(
            crate::caught::message(payload.as_ref()).to_owned(),
        )),
    }
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

/// The longest message kept: this runs on error paths to word another error,
/// and an exception's message is untrusted input.
const EXCEPTION_MESSAGE_MAX: usize = 255;

/// `exc`'s message, for wording another error with it: up to the first NUL,
/// at most [`EXCEPTION_MESSAGE_MAX`] bytes, invalid UTF-8 replaced. "error"
/// for a message that is not a String - this runs on error paths, so it must
/// not raise itself.
pub fn exception_message(exc: VALUE) -> String {
    /* The message is the exception's `mesg`, read as an attribute: calling
     * `#message` would run its class's Ruby. A message that is not a String
     * is answered as "error". */
    // SAFETY: a NUL-terminated name.
    let Ok(id) = (unsafe { super::raw::intern_cstr(c"mesg".as_ptr()) }) else {
        return "error".to_owned();
    };
    // SAFETY: `exc` is a live exception; reading an attribute runs no Ruby
    // and raises nothing.
    let msg = unsafe { rb_sys::rb_attr_get(exc, id) };
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
