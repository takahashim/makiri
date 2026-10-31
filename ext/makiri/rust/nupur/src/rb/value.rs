//! Ruby's values, as types: [`Value`] and the class-checked handles over it.
//!
//! Each is a `VALUE` and nothing else - `Copy`, the size of a pointer, and
//! neither `Send` nor `Sync`, since a Ruby value is only good on a Ruby thread
//! holding the GVL. A handle (`RString`, `RHash`, ...) is made only by its
//! `from_value`, which reads the value's type from its bits and header and so
//! runs no Ruby and raises nothing; a method that could do either goes through
//! `raw`'s protected calls.
//!
//! Like any Ruby value in an extension, one is good while the GC can find it:
//! on the machine stack (the conservative scan), or in a wrapped object's slot
//! (`typed::TypedType`'s `roots`). `rake dylint`'s `ruby_value_on_heap` reports one
//! kept anywhere else, by the [`ReprValue`] these implement.

#![allow(unsafe_code)]

use core::marker::PhantomData;

use rb_sys::stable_api::{get_default, StableApiDefinition};
use rb_sys::{ruby_value_type, VALUE};

use super::error::{Builtin, Error};
use super::object::{class_inherits, obj_is_kind_of};
use super::ruby::Ruby;

// What a String's header says of it: its coderange, encoding index, class.
mod header;
pub use header::Coderange;

/// Any Ruby value.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Value(VALUE, PhantomData<*mut ()>);

impl Value {
    /// The value a C call returned, or one Ruby passed in.
    ///
    /// # Safety
    /// `v` is a live Ruby value, kept where the GC finds it for as long as the
    /// `Value` is used.
    #[inline]
    pub(crate) const unsafe fn from_raw(v: VALUE) -> Value {
        Value(v, PhantomData)
    }
}

/// What every Ruby value type is: a `Value` underneath.
///
/// The lint `ruby_value_on_heap` finds Ruby values by this trait, so every
/// type that holds one directly implements it.
pub trait ReprValue: Copy {
    /// The plain value.
    fn as_value(self) -> Value;

    /// `nil`?
    #[inline]
    fn is_nil(self) -> bool {
        get_default().nil_p(self.as_value().0)
    }

    /// Ruby's truthiness: anything but `nil` and `false`.
    #[inline]
    fn to_bool(self) -> bool {
        get_default().rb_test(self.as_value().0)
    }

    /// Frozen? An immediate (a Fixnum, a Symbol, `nil`) is always frozen.
    #[inline]
    fn is_frozen(self) -> bool {
        let v = self.as_value().0;
        let api = get_default();
        // SAFETY: `v` is a live value; a non-immediate has a header to read.
        api.special_const_p(v) || unsafe { api.frozen_p(v) }
    }

    /// `Ok` unfrozen, and Ruby's own `FrozenError` frozen - the message
    /// naming the receiver, `#receiver` set - returned, not raised. The
    /// normal case, an unfrozen value, makes no call.
    #[inline]
    fn check_frozen(self) -> Result<(), Error> {
        crate::raw::check_frozen(self.as_value())
    }

    /// An instance of `class` or of a class deriving from it? Reads the
    /// ancestry, runs no Ruby (`rb_obj_is_kind_of`).
    #[inline]
    fn is_kind_of(self, class: impl ReprValue) -> bool {
        obj_is_kind_of(self.as_value(), class.as_value())
    }

    /// The name of the value's class, for a message, written into `f` -
    /// "Object" when the name cannot be made (an anonymous class's is a fresh
    /// String, and so an allocation that can fail).
    fn write_classname(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.as_value();
        crate::describe::for_message(|| {
            crate::raw::with_classname(v, |name| {
                core::fmt::Display::fmt(&crate::msg::Lossy(name), &mut *f)
            })
        })
        .unwrap_or_else(|| f.write_str("Object"))
    }
}

impl ReprValue for Value {
    #[inline]
    fn as_value(self) -> Value {
        self
    }
}

/// The raw `VALUE`, for the layer's C calls and nothing else.
pub(crate) trait AsRawValue {
    #[allow(clippy::wrong_self_convention, reason = "a Copy value, read by value")]
    fn as_raw(self) -> VALUE;
}

impl<T: ReprValue> AsRawValue for T {
    #[inline]
    fn as_raw(self) -> VALUE {
        self.as_value().0
    }
}

/// The value of a type `rb_type` names.
#[inline]
fn of_type(v: Value, t: ruby_value_type) -> bool {
    // SAFETY: `rb_type` reads an immediate's bits or a live object's header.
    unsafe { get_default().rb_type(v.0) == t }
}

macro_rules! handle {
    ($(#[$doc:meta])* $name:ident, $test:expr) => {
        $(#[$doc])*
        #[derive(Clone, Copy)]
        #[repr(transparent)]
        pub struct $name(Value);

        impl $name {
            /// `v` as this type, or `None` when it is something else.
            #[inline]
            pub fn from_value(v: Value) -> Option<$name> {
                let test: fn(Value) -> bool = $test;
                test(v).then_some($name(v))
            }

            /// `v`, which the caller knows to be one: a value this layer
            /// made as one, or a Ruby global of that kind.
            #[inline]
            #[allow(dead_code)]
            pub(crate) fn from_known(v: Value) -> $name {
                debug_assert!({
                    let test: fn(Value) -> bool = $test;
                    test(v)
                });
                $name(v)
            }
        }

        impl ReprValue for $name {
            #[inline]
            fn as_value(self) -> Value {
                self.0
            }
        }
    };
}

handle!(
    /// A String.
    RString,
    |v| of_type(v, ruby_value_type::RUBY_T_STRING)
);
handle!(
    /// An Array.
    RArray,
    |v| of_type(v, ruby_value_type::RUBY_T_ARRAY)
);
handle!(
    /// A Hash.
    RHash,
    |v| of_type(v, ruby_value_type::RUBY_T_HASH)
);
handle!(
    /// A Symbol, static or dynamic.
    Symbol,
    // SAFETY: as `of_type`.
    |v| unsafe { get_default().symbol_p(v.0) }
);
handle!(
    /// An Integer: a Fixnum or a Bignum.
    Integer,
    // SAFETY: as `of_type`.
    |v| unsafe { get_default().integer_type_p(v.0) }
);
handle!(
    /// A Float.
    Float,
    // SAFETY: as `of_type`.
    |v| unsafe { get_default().float_type_p(v.0) }
);
handle!(
    /// A Range (or an instance of a subclass).
    Range,
    // SAFETY: `rb_cRange`, a C global Ruby sets once at boot; a value
    // existing means this is a Ruby thread, and the test reads its ancestry.
    |v| v.is_kind_of(RClass::from_known(unsafe { Value::from_raw(rb_sys::rb_cRange) }))
);
handle!(
    /// A Class.
    RClass,
    |v| of_type(v, ruby_value_type::RUBY_T_CLASS)
);
handle!(
    /// A Module that is not a Class.
    RModule,
    |v| of_type(v, ruby_value_type::RUBY_T_MODULE)
);
handle!(
    /// A Class deriving from `Exception`, for raising.
    ExceptionClass,
    /* `rb_eException` read directly, not through `Ruby::exception_exception`:
     * that is an `ExceptionClass::from_known`, whose debug assertion is this
     * test, so a debug build would recurse without end. */
    |v| of_type(v, ruby_value_type::RUBY_T_CLASS)
        // SAFETY: a C global Ruby sets once at boot, read under the GVL.
        && class_inherits(v, unsafe { Value::from_raw(rb_sys::rb_eException) })
);

impl RString {
    /// The length in bytes.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live String.
        crate::int::len(unsafe { get_default().rstring_len(self.0 .0) })
    }

    /// Empty?
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The bytes, borrowed from the String.
    ///
    /// # Safety
    /// The slice is good only while nothing can move, grow, shrink or free
    /// the String's buffer: no Ruby code and no allocation may run while it
    /// is held.
    #[inline]
    pub(crate) unsafe fn as_slice(&self) -> &[u8] {
        let api = get_default();
        // SAFETY: `self` is a live String; the caller's contract.
        unsafe {
            let (p, n) = (api.rstring_ptr(self.0 .0), api.rstring_len(self.0 .0));
            let n = crate::int::len(n);
            if n == 0 || p.is_null() {
                return &[];
            }
            core::slice::from_raw_parts(p as *const u8, n)
        }
    }
}

impl RArray {
    /// The length.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live Array.
        crate::int::len(unsafe { get_default().rarray_len(self.0 .0) })
    }

    /// Empty?
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Element `i`, `nil` past the end: `rb_ary_entry`, which raises
    /// nothing.
    #[inline]
    pub fn entry(self, i: usize) -> Value {
        ary_entry(self, i)
    }

    /// The elements, read one at a time (`rb_ary_entry`), the length read
    /// afresh before each.
    pub fn each(self) -> impl Iterator<Item = Value> {
        (0..).map_while(move |i: usize| (i < self.len()).then(|| ary_entry(self, i)))
    }
}

impl RHash {
    /// The number of pairs.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live Hash.
        unsafe { get_default().rhash_size(self.0 .0) }
    }

    /// Empty?
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The value at `key`, or `None` when there is none. A key is a Symbol
    /// or a String ([`HashKey`]), which Ruby hashes and compares in C; a key
    /// of the Hash's own that collides with it is compared by its `eql?`,
    /// which can raise - `Err`, passed on (`raw::hash_lookup`).
    #[inline]
    pub fn get(self, key: impl HashKey) -> Result<Option<Value>, Error> {
        crate::raw::hash_lookup(self, key.as_value())
    }
}

/// A Hash key whose lookup runs no Ruby: hashed and compared in C. A String
/// is one only when its class is String itself (`plain_key`): Ruby takes
/// the C path for two Strings only then, and calls `eql?` - the caller's
/// Ruby - for a subclass's instance, so the lookup refuses one.
pub trait HashKey: ReprValue {}
impl HashKey for Symbol {}
impl HashKey for RString {}

/// Whether `k` is a key Ruby hashes and compares in C: a Symbol, or a String
/// whose class is String itself. Reads the header; runs no Ruby.
#[inline]
pub(crate) fn plain_key(k: Value) -> bool {
    if Symbol::from_value(k).is_some() {
        return true;
    }
    RString::from_value(k).is_some_and(RString::has_string_class)
}

impl Range {
    /// The first end, the last end and whether the last is excluded.
    #[inline]
    pub fn values(self) -> Result<(Value, Value, bool), Error> {
        crate::raw::range_values(self)
    }

    /// The part of a collection of `count` elements this Range selects, as
    /// `Array#[]` reads one (`rb_range_beg_len`): the start and the length,
    /// `None` for a start outside the collection, `RangeError` for a bound
    /// too large, and a `TypeError` for an end that is not an Integer or
    /// nil - whose `to_int` the C would run.
    #[inline]
    pub fn beg_len(self, count: usize) -> Result<Option<(usize, usize)>, Error> {
        crate::raw::range_beg_len(self, count)
    }
}

impl Integer {
    /// As a `u64`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_u64(self) -> Result<u64, Error> {
        if self.is_negative() {
            return Err(Error::new(
                Builtin::RangeError,
                "can't convert negative integer to unsigned",
            ));
        }
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            /* Not negative, checked above. */
            return u64::try_from(i64::from(api.fix2long(self.0 .0)))
                .map_err(|_| crate::rb::error::internal_error("a negative Fixnum"));
        }
        crate::raw::big2ull(self)
    }

    /// As an `i64`; a value out of its range is Ruby's `RangeError`.
    #[inline]
    pub fn to_i64(self) -> Result<i64, Error> {
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            return Ok(i64::from(api.fix2long(self.0 .0)));
        }
        crate::raw::big2ll(self)
    }

    /// As a `usize`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_usize(self) -> Result<usize, Error> {
        /* Through `to_u64`, so a value past a narrower `usize` is refused
         * rather than truncated. */
        usize::try_from(self.to_u64()?).map_err(|_| {
            Error::new(
                Builtin::RangeError,
                "integer too big to convert into a usize",
            )
        })
    }

    /// As a `usize`, or `None` when it is negative or past one - and only
    /// then: any other failure of the conversion is passed on, never read as
    /// "out of range" (for a limit, that would lift it). "Past one" is
    /// decided from the value's size, so no `RangeError` is made and dropped.
    pub fn to_usize_within(self) -> Result<Option<usize>, Error> {
        if self.is_negative() {
            return Ok(None);
        }
        if !get_default().fixnum_p(self.0 .0) && !self.magnitude_fits_u64() {
            return Ok(None);
        }
        Ok(usize::try_from(self.to_u64()?).ok())
    }

    /// Whether a Bignum's magnitude fits a `u64`: `rb_absint_size`, which
    /// reads its digits and raises nothing - so "too large" is answered
    /// without making the `RangeError` `rb_big2ull` would raise.
    #[inline]
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raising_call_outside_protect,
            reason = "its raise is for a non-Integer, converted by `#to_int`; `self` is an Integer"
        )
    )]
    fn magnitude_fits_u64(self) -> bool {
        // SAFETY: `self` is a live Integer; the C reads its digits.
        let bytes = unsafe { rb_sys::rb_absint_size(self.0 .0, core::ptr::null_mut()) };
        u64::try_from(bytes).is_ok_and(|b| b <= 8)
    }

    /// Below zero? Reads the sign, converts nothing.
    #[inline]
    pub fn is_negative(self) -> bool {
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            return api.fix2long(self.0 .0) < 0;
        }
        // SAFETY: a non-Fixnum Integer is a live Bignum.
        unsafe { api.bignum_negative_p(self.0 .0) }
    }
}

impl Float {
    /// As an `f64`.
    #[inline]
    pub fn to_f64(self) -> f64 {
        // SAFETY: `self` is a live Float.
        unsafe { get_default().num2dbl(self.0 .0) }
    }
}

/* ---- immediates and identity ---- */

/// `nil`, as a `Value`.
#[inline]
pub fn nil(ruby: &Ruby) -> Value {
    ruby.qnil().as_value()
}

/// `true`/`false`, as a `Value`.
#[inline]
pub fn boolean(ruby: &Ruby, b: bool) -> Value {
    if b {
        ruby.qtrue().as_value()
    } else {
        ruby.qfalse().as_value()
    }
}

/// VALUE identity, for the several sites that compare two references.
#[inline]
pub fn same_value(a: impl ReprValue, b: impl ReprValue) -> bool {
    a.as_raw() == b.as_raw()
}

/// VALUE identity as a word: the same for every reference to one object, and
/// different for two live objects - a key to compare by, never a value to
/// turn back into one.
#[inline]
pub fn identity_word(v: impl ReprValue) -> usize {
    crate::int::word(v.as_raw())
}

/// An interned name - a method's, an ivar's, a constant's - as `rb_intern`
/// makes it (`raw::intern`). Only the layer turns one into the raw `ID` it
/// hands C, so a caller can keep and compare one but never forge one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct Id(rb_sys::ID);

impl Id {
    /// The interned name `name` (`rb_intern`); a new name grows the symbol
    /// table, so it can raise `NoMemoryError`.
    pub fn intern(ruby: &Ruby, name: &core::ffi::CStr) -> Result<Id, Error> {
        crate::raw::intern_cstr(ruby, name)
    }

    #[inline]
    pub(crate) const fn from_raw(id: rb_sys::ID) -> Id {
        Id(id)
    }

    #[inline]
    pub(crate) const fn as_raw(self) -> rb_sys::ID {
        self.0
    }
}

/// The name of a hidden instance variable: an [`Id`] interned from a name
/// that does not start with `@`, so no Ruby code can read or write it.
///
/// What the ivar reads and writes take ([`crate::rb::object::ivar_get`],
/// [`crate::rb::object::ivar_set`]): Ruby's Ractor checks on an ivar - of a
/// class or module, or of a shareable object, from another Ractor - apply to
/// an `@` name alone (`rb_is_instance_id`, in every release from 3.3), so
/// with one of these the read raises nothing in any Ractor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct HiddenId(Id);

impl HiddenId {
    /// The hidden name `name` (`rb_intern`): `ArgumentError` for one that
    /// starts with `@`, and `NoMemoryError` as [`Id::intern`].
    pub fn intern(ruby: &Ruby, name: &core::ffi::CStr) -> Result<HiddenId, Error> {
        if name.to_bytes().first() == Some(&b'@') {
            return Err(Error::new(
                crate::rb::Builtin::ArgumentError,
                "a hidden ivar's name does not start with @",
            ));
        }
        Id::intern(ruby, name).map(HiddenId)
    }

    #[inline]
    pub(crate) const fn as_raw(self) -> rb_sys::ID {
        self.0.as_raw()
    }
}

/* ---- a C read that raises nothing (so it is not `raw`'s, which holds the
 * calls that can) ---- */

/// `rb_ary_entry`: element `i`, `nil` past the end. Raises nothing.
#[inline]
pub(crate) fn ary_entry(a: RArray, i: usize) -> Value {
    /* Converted, not cut: a `long` is 32 bits on Windows, where an index
     * past one would wrap to an element that is there. No Array has one. */
    let Ok(i) = i.try_into() else {
        // SAFETY: `nil`, an immediate.
        return unsafe { Value::from_raw(rb_sys::Qnil as VALUE) };
    };
    // SAFETY: `a` is a live Array; the read is bounds-checked by Ruby.
    unsafe { Value::from_raw(rb_sys::rb_ary_entry(a.as_raw(), i)) }
}
