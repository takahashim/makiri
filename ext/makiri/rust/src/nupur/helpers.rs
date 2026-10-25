//! The Ruby layer's shared helpers: argument checks, the common exceptions,
//! and the raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are built on `bridge::raw`'s wrappers, each of which turns
//! the raise into a [`crate::nupur::rb::Error`]: the caller hands it back as
//! `Err`, and the method's trampoline raises it only after the Rust frames
//! have returned normally (`bridge::rb::method::finish`).

#![allow(unsafe_code)]

use core::ffi::c_long;
use std::sync::OnceLock;

use crate::nupur::rb::value::AsRawValue;
use crate::nupur::rb::{prelude::*, Error, ExceptionClass, Opaque, RString, Ruby, Value};

/// The raw handle types, re-exported so a higher layer can name a `VALUE` or a
/// method `ID` without reaching into `rb_sys` itself.
pub use rb_sys::ID;

/* The two conveniences every bridge and glue module shares. */

/// The `MAJOR.MINOR` of the Ruby whose headers this extension was compiled
/// against - its ABI, since a teeny release keeps it. Published as
/// `Makiri::NATIVE_RUBY_API_VERSION`.
pub fn ruby_api_version() -> crate::nupur::msg::Msg {
    crate::nupur::msg::Msg::format(format_args!(
        "{}.{}",
        rb_sys::RUBY_API_VERSION_MAJOR,
        rb_sys::RUBY_API_VERSION_MINOR
    ))
}

/// The Ruby API `MAJOR.MINOR` this extension was compiled against.
pub fn built_ruby_api() -> (core::ffi::c_int, core::ffi::c_int) {
    (
        rb_sys::RUBY_API_VERSION_MAJOR as core::ffi::c_int,
        rb_sys::RUBY_API_VERSION_MINOR as core::ffi::c_int,
    )
}

/// The running Ruby's API `MAJOR.MINOR`, read from the `ruby_api_version`
/// it exports (three ints, whose layout no Ruby changes).
pub fn running_ruby_api() -> (core::ffi::c_int, core::ffi::c_int) {
    // SAFETY: an immutable `const int[3]` libruby exports, read by value.
    unsafe { (rb_sys::ruby_api_version[0], rb_sys::ruby_api_version[1]) }
}

/// `Makiri::InternalError`, or whichever class the extension registers
/// ([`RbConst::set`]): what a broken invariant of the layer raises.
pub static INTERNAL_ERROR: RbConst<ExceptionClass> = RbConst::new();

/// An internal error carrying `msg`: a broken invariant, which a bare
/// `rescue` (StandardError) passes through - the registered class
/// ([`INTERNAL_ERROR`]), or `RuntimeError` before one is.
pub fn internal_error(msg: impl crate::nupur::rb::Message) -> Error {
    Error::new(INTERNAL_ERROR.exception(), msg)
}

/// Is `v` an instance of the class in `klass`? `false` before `Init_makiri`,
/// when no class of ours exists for it to be an instance of.
#[inline]
pub fn is_kind_of(v: Value, klass: &RbConst<crate::nupur::rb::RClass>) -> bool {
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
pub fn integer_arg(v: Value) -> Result<crate::nupur::rb::Integer, Error> {
    crate::nupur::rb::Integer::from_value(v).ok_or_else(|| wrong_type(v, "Integer", TO_I_HINT))
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
    let r = crate::nupur::rb::Range::from_value(v).ok_or_else(|| wrong_type(v, "Range", ""))?;
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
    match crate::nupur::rb::Symbol::from_value(v) {
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
        if let Some(sym) = crate::nupur::rb::Symbol::from_value(v) {
            if let Ok(s) = super::raw::sym2str(sym) {
                // SAFETY: written out at once; formatting runs no Ruby.
                return write!(f, ":{}", crate::nupur::msg::Lossy(unsafe { s.as_slice() }));
            }
        }
        if let Some(s) = RString::from_value(v) {
            // SAFETY: as above.
            return crate::nupur::msg::Quoted(unsafe { s.as_slice() }).fmt(f);
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
        let text = match crate::nupur::rb::Symbol::from_value(v) {
            Some(sym) => super::raw::sym2str(sym),
            None => match RString::from_value(v) {
                Some(s) => Ok(s),
                None => return Describe(v).fmt(f),
            },
        };
        if let Ok(s) = text {
            // SAFETY: written out at once; formatting runs no Ruby.
            return crate::nupur::msg::Lossy(unsafe { s.as_slice() }).fmt(f);
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
    if !crate::nupur::rb::ReprValue::is_frozen(v) {
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
pub fn type_error(msg: impl crate::nupur::rb::Message) -> Error {
    Error::new(gvl_ruby().exception_type_error(), msg)
}

/// An `ArgumentError` with `msg`.
pub fn arg_error(msg: impl crate::nupur::rb::Message) -> Error {
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
pub fn reveal(v: Value, klass: crate::nupur::rb::RClass) {
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
        crate::nupur::msg::Lossy(&bytes[..n]).fmt(f)
    }
}

/// A class, module or exception class `init` defines, for the glue to read.
///
/// Typed: an `RbConst<RClass>` can only ever hold an `RClass`, so a read hands
/// the handle back with no type check to fail. It is written once, by `init`,
/// which also registers the object with the GC - so it stays alive and unmoved
/// even if Ruby code removes the constant that named it.
///
/// Unset is the state before `Init_makiri`, when none of the methods that read
/// one exist yet. A read in that state answers `None` (or the documented
/// fallback of the reader below) rather than panicking.
pub struct RbConst<T>(OnceLock<Opaque<T>>);

impl<T: ReprValue> RbConst<T> {
    pub(crate) const fn new() -> RbConst<T> {
        RbConst(OnceLock::new())
    }

    /// Record `v`, rooted for the rest of the process. `init` is the only
    /// writer, and a second write is refused rather than replacing the first.
    pub fn set(&self, v: T) -> Result<(), Error> {
        self.0.set(Opaque::register(v)?).map_err(|_| {
            Error::new(
                gvl_ruby().exception_runtime_error(),
                "a constant handle was set twice",
            )
        })
    }

    /// The object, or `None` before `Init_makiri`.
    #[inline]
    pub fn get(&self) -> Option<T> {
        self.0.get().map(|o| gvl_ruby().get_inner(*o))
    }

    /// The object, for the registration code `init` runs once it is set.
    pub fn defined(&self) -> Result<T, Error> {
        self.get().ok_or_else(|| {
            Error::new(
                gvl_ruby().exception_runtime_error(),
                "a constant handle was read before the extension initialised",
            )
        })
    }
}

impl RbConst<ExceptionClass> {
    /// The class to raise. Before `Init_makiri` - when nothing that raises one
    /// of these can run - it is `RuntimeError`, so a raise still raises.
    #[inline]
    pub fn exception(&self) -> ExceptionClass {
        self.get()
            .unwrap_or_else(|| gvl_ruby().exception_runtime_error())
    }
}
