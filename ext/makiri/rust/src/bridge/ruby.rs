//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are built on `bridge::raw`'s wrappers, each of which turns
//! the raise into a [`crate::bridge::rb::Error`]: the caller hands it back as
//! `Err`, and the method's trampoline raises it only after the Rust frames
//! have returned normally (`bridge::rb::method::finish`).

#![allow(unsafe_code)]

use core::ffi::c_long;

use crate::bridge::rb::value::AsRawValue;
use crate::bridge::rb::{prelude::*, Error, RString, Ruby, Value};

/// The raw handle types, re-exported so a higher layer can name a `VALUE` or a
/// method `ID` without reaching into `rb_sys` itself.
pub use rb_sys::ID;

/* The two conveniences every bridge and glue module shares. */

/// The `MAJOR.MINOR` of the Ruby whose headers this extension was compiled
/// against - its ABI, since a teeny release keeps it. Published as
/// `Makiri::NATIVE_RUBY_API_VERSION`.
pub fn ruby_api_version() -> crate::msg::Msg {
    crate::msg::Msg::format(format_args!(
        "{}.{}",
        rb_sys::RUBY_API_VERSION_MAJOR,
        rb_sys::RUBY_API_VERSION_MINOR
    ))
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
        format_args!(
            "makiri's native extension was built for Ruby {}.{}, but this is Ruby {}.{}; \
             rebuild it (`bundle exec rake clean compile` in a checkout, or reinstall the gem)",
            built.0, built.1, running.0, running.1
        ),
    ))
}

/// `Makiri::Error` - the one definition; the modules that each kept a private
/// copy of this now import it.
pub fn error_class() -> crate::bridge::rb::ExceptionClass {
    crate::init::EXC_ERROR.exception()
}

/// A `Makiri::Error` carrying `msg` - the one way the extension raises its own
/// error class, so a caller never spells the class (or finds a stale copy of
/// the lookup) itself.
pub fn makiri_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(error_class(), msg)
}

/// A `Makiri::InternalError` carrying `msg`: a broken invariant of Makiri
/// itself, which a bare `rescue` (StandardError) passes through.
pub fn internal_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(crate::init::EXC_INTERNAL_ERROR.exception(), msg)
}

/// Is `v` an instance of the class in `klass`? `false` before `Init_makiri`,
/// when no class of ours exists for it to be an instance of.
#[inline]
pub fn is_kind_of(v: Value, klass: &crate::init::RbConst<crate::bridge::rb::RClass>) -> bool {
    klass.get().is_some_and(|c| v.is_kind_of(c))
}

/// `v`, which must be a String (a subclass's instance included).
///
/// There is no conversion: a `to_str` or `to_s` is the caller's Ruby code, and
/// Makiri runs none, so anything else is a `TypeError`, worded as Ruby's own
/// type check words it.
pub fn string_arg(v: Value) -> Result<RString, Error> {
    RString::from_value(v).ok_or_else(|| wrong_type(v, "String", TO_S_HINT))
}

/// `v`, which must be an Integer (a Fixnum or a Bignum). No conversion, as
/// [`string_arg`]: a `to_int` is the caller's Ruby.
pub fn integer_arg(v: Value) -> Result<crate::bridge::rb::Integer, Error> {
    crate::bridge::rb::Integer::from_value(v).ok_or_else(|| wrong_type(v, "Integer", TO_I_HINT))
}

/// [`integer_arg`] as a `long`; one past a `long` is Ruby's `RangeError`.
pub fn long_arg(v: Value) -> Result<c_long, Error> {
    let n = integer_arg(v)?.to_i64()?;
    c_long::try_from(n).map_err(|_| {
        Error::new(
            gvl_ruby().exception_range_error(),
            format_args!("integer {n} too big for a long"),
        )
    })
}

/// A Range argument whose ends are Integers (or nil, for an endless or
/// beginless one): `rb_range_beg_len` would otherwise convert an end with its
/// `to_int`.
pub fn integer_range_arg(v: Value) -> Result<Value, Error> {
    let r = crate::bridge::rb::Range::from_value(v).ok_or_else(|| wrong_type(v, "Range", ""))?;
    let (beg, end, _) = r.values()?;
    for end in [beg, end] {
        if !end.is_nil() {
            integer_arg(end)?;
        }
    }
    Ok(v)
}

const TO_S_HINT: &str = "; call #to_s to convert it";
const TO_I_HINT: &str = "; call #to_i to convert it";

/// `v` as a String for a NAME (an attribute or element name, a prefix, a
/// selector, an expression): a String, or a Symbol's name. Nothing else
/// converts, as [`string_arg`].
pub fn name_arg(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        return Ok(s);
    }
    match crate::bridge::rb::Symbol::from_value(v) {
        Some(sym) => super::raw::sym2str(sym),
        None => Err(wrong_type(v, "String or Symbol", TO_S_HINT)),
    }
}

/// `v` for an error message, written without calling it: a Symbol as
/// `:name`, a String quoted, anything else by its class (`#inspect` would be
/// the caller's Ruby).
pub struct Describe(pub Value);

impl core::fmt::Display for Describe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.0;
        if let Some(sym) = crate::bridge::rb::Symbol::from_value(v) {
            if let Ok(s) = super::raw::sym2str(sym) {
                // SAFETY: written out at once; formatting runs no Ruby.
                return write!(f, ":{}", crate::msg::Lossy(unsafe { s.as_slice() }));
            }
        }
        if let Some(s) = RString::from_value(v) {
            // SAFETY: as above.
            return crate::msg::Quoted(unsafe { s.as_slice() }).fmt(f);
        }
        if v.is_nil() {
            return f.write_str("nil");
        }
        f.write_str("an instance of ")?;
        v.write_classname(f)
    }
}

/// `v` as text for a message, as Ruby's `to_s` would have it for the values a
/// message holds - a String's text, a Symbol's name - but without calling it
/// (`to_s` is the caller's Ruby); anything else as [`Describe`] has it.
pub struct Display(pub Value);

impl core::fmt::Display for Display {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.0;
        /* The String or the Symbol's name, read directly: `name_arg` would
         * build (and format) a whole TypeError for any other value, only for
         * it to be thrown away here. */
        let text = match crate::bridge::rb::Symbol::from_value(v) {
            Some(sym) => super::raw::sym2str(sym),
            None => match RString::from_value(v) {
                Some(s) => Ok(s),
                None => return Describe(v).fmt(f),
            },
        };
        if let Ok(s) = text {
            // SAFETY: written out at once; formatting runs no Ruby.
            return crate::msg::Lossy(unsafe { s.as_slice() }).fmt(f);
        }
        Describe(v).fmt(f)
    }
}

/// The class name of a value, for a message.
struct ClassName(Value);

impl core::fmt::Display for ClassName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.write_classname(f)
    }
}

/// Ruby's `TypeError` for a value of the wrong class, worded as Ruby's own
/// type check words it, with the way out (`hint`): "wrong argument type
/// Integer (expected String); call #to_s to convert it". Makiri converts
/// nothing itself (a `to_s` is the caller's Ruby), so the caller does. The
/// class name is read without a call.
fn wrong_type(v: Value, expected: &str, hint: &str) -> Error {
    type_error(format_args!(
        "wrong argument type {} (expected {expected}){hint}",
        ClassName(v)
    ))
}

/// `rb_check_frozen` returning its FrozenError rather than raising it.
///
/// The error is Ruby's own - the message naming the receiver and `#receiver`
/// set - because the check still runs, protected. An unfrozen value, every
/// mutator's normal case, makes no call.
pub fn check_frozen(v: Value) -> Result<(), Error> {
    if !crate::bridge::rb::ReprValue::is_frozen(v) {
        return Ok(());
    }
    super::raw::check_frozen(v)
}

/// `rb_range_beg_len` over a collection of `count` elements, with the C's
/// `err = 0`: a start outside the collection is `None` rather than a raise.
///
/// A bound too large for a `long` is `Err` (`RangeError`).
pub fn range_beg_len(range: Value, count: c_long) -> Result<Option<(c_long, c_long)>, Error> {
    super::raw::range_beg_len(range, count)
}

/* ------------------------------------------------------------------ *
 * Value-level helpers                                                *
 * ------------------------------------------------------------------ */

/* The singletons the glue would otherwise spell as rb_sys constants. `nil`
 * and `boolean` are immortal and cannot raise; what allocates is
 * `bridge::build`'s. */

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

/// A `TypeError` with `msg`.
pub fn type_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(gvl_ruby().exception_type_error(), msg)
}

/// An `ArgumentError` with `msg`.
pub fn arg_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(gvl_ruby().exception_arg_error(), msg)
}

/// VALUE identity, for the several sites that compare two references.
#[inline]
pub fn same_value(a: Value, b: Value) -> bool {
    a.as_raw() == b.as_raw()
}

/// Hide `v` from `ObjectSpace` (`rb_obj_hide`: its class field cleared), as
/// a Document is until it has content. Raises nothing.
pub fn hide(v: Value) {
    // SAFETY: a live object of ours, by `Value`'s rule; hiding writes only its
    // class field.
    unsafe { rb_sys::rb_obj_hide(v.as_raw()) };
}

/// Show `v` again with `klass`, the class it was made with (`rb_obj_reveal`).
/// Raises nothing.
pub fn reveal(v: Value, klass: crate::bridge::rb::RClass) {
    // SAFETY: as `hide`; `klass` is a live class, by type.
    unsafe { rb_sys::rb_obj_reveal(v.as_raw(), klass.as_raw()) };
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
pub struct ExceptionText(pub Value);

impl core::fmt::Display for ExceptionText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        /* The message is the exception's `mesg`, read as an attribute: calling
         * `#message` would run its class's Ruby. A message that is not a
         * String is answered as "error". */
        let Ok(id) = super::raw::intern_cstr(&gvl_ruby(), c"mesg") else {
            return f.write_str("error");
        };
        /* An attribute read: it runs no Ruby and raises nothing. */
        let Some(r) = RString::from_value(super::raw::ivar_get(self.0, id)) else {
            return f.write_str("error");
        };
        // SAFETY: written out at once; formatting runs no Ruby.
        let bytes = unsafe { r.as_slice() };
        let n = bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(bytes.len())
            .min(EXCEPTION_MESSAGE_MAX);
        crate::msg::Lossy(&bytes[..n]).fmt(f)
    }
}
