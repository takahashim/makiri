//! Ruby's values, as types: [`Value`] and the class-checked handles over it.
//!
//! Each is a `VALUE` and nothing else - `Copy`, the size of a pointer, and
//! neither `Send` nor `Sync`, since a Ruby value is only good on a Ruby thread
//! holding the GVL. A handle (`RString`, `RHash`, ...) is made only by its
//! `from_value`, which reads the value's type from its bits and header and so
//! runs no Ruby and raises nothing; a method that could do either goes through
//! `bridge::raw`'s protected calls.
//!
//! Like any Ruby value in an extension, one is good while the GC can find it:
//! on the machine stack (the conservative scan), or in a field a mark traces
//! (`bridge::typed::Held`). `rake dylint`'s `ruby_value_on_heap` reports one
//! kept anywhere else, by the [`ReprValue`] these implement.

use core::ffi::{c_long, CStr};
use core::marker::PhantomData;

use rb_sys::stable_api::{get_default, StableApiDefinition};
use rb_sys::{ruby_value_type, VALUE};

use super::error::Error;
use super::ruby::Ruby;

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
    pub(in crate::bridge) const unsafe fn from_raw(v: VALUE) -> Value {
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

    /// An instance of `class` or of a class deriving from it? Reads the
    /// ancestry, runs no Ruby (`rb_obj_is_kind_of`).
    #[inline]
    fn is_kind_of(self, class: impl ReprValue) -> bool {
        crate::bridge::raw::obj_is_kind_of(self.as_value(), class.as_value())
    }

    /// The name of the value's class, for a message, written into `f`.
    ///
    /// # Safety
    /// The value must be live. The name is Ruby's own C string, owned by the
    /// class, and is read here with nothing in between that runs Ruby.
    unsafe fn write_classname(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // SAFETY: `rb_obj_classname` returns a C string owned by the class,
        // live while the value is; written out at once.
        unsafe {
            let p = rb_sys::rb_obj_classname(self.as_value().0);
            if p.is_null() {
                return f.write_str("Object");
            }
            core::fmt::Display::fmt(&crate::msg::Lossy(CStr::from_ptr(p).to_bytes()), f)
        }
    }
}

impl ReprValue for Value {
    #[inline]
    fn as_value(self) -> Value {
        self
    }
}

/// The raw `VALUE`, for `bridge`'s C calls and nothing else.
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
            pub(in crate::bridge) fn from_known(v: Value) -> $name {
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
    |v| v.is_kind_of(Ruby::get_with(v).class_range())
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
    |v| of_type(v, ruby_value_type::RUBY_T_CLASS)
        && crate::bridge::raw::class_inherits(v, Ruby::get_with(v).exception_exception().as_value())
);

impl RString {
    /// The length in bytes.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live String.
        unsafe { get_default().rstring_len(self.0 .0) as usize }
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
    pub unsafe fn as_slice(&self) -> &[u8] {
        let api = get_default();
        // SAFETY: `self` is a live String; the caller's contract.
        unsafe {
            let (p, n) = (api.rstring_ptr(self.0 .0), api.rstring_len(self.0 .0));
            if n <= 0 || p.is_null() {
                return &[];
            }
            core::slice::from_raw_parts(p as *const u8, n as usize)
        }
    }

    /// The String's cached coderange - a read of its header flags, which
    /// never scans and never allocates.
    #[inline]
    pub fn enc_coderange(self) -> Coderange {
        const MASK: VALUE = rb_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_MASK as VALUE;
        const SEVEN: VALUE = rb_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_7BIT as VALUE;
        const VALID: VALUE = rb_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_VALID as VALUE;
        const BROKEN: VALUE = rb_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_BROKEN as VALUE;
        // SAFETY: a String is an object, whose header starts with `RBasic`.
        let flags = unsafe { (*(self.0 .0 as *const rb_sys::RBasic)).flags };
        match flags & MASK {
            SEVEN => Coderange::SevenBit,
            VALID => Coderange::Valid,
            BROKEN => Coderange::Broken,
            _ => Coderange::Unknown,
        }
    }
}

/// A String's coderange, as Ruby caches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coderange {
    Unknown,
    SevenBit,
    Valid,
    Broken,
}

impl RArray {
    /// The length.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live Array.
        unsafe { get_default().rarray_len(self.0 .0) as usize }
    }

    /// Empty?
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Element `i` (negative counts from the end), `nil` past the end:
    /// `rb_ary_entry`, which raises nothing.
    #[inline]
    pub fn entry(self, i: isize) -> Result<Value, Error> {
        Ok(crate::bridge::raw::ary_entry(self, i as c_long))
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
    /// or a String ([`HashKey`]), which Ruby hashes and compares in C, so
    /// the lookup runs no Ruby code of the caller's (a key of another kind
    /// could: its `hash` and `eql?`).
    #[inline]
    pub fn get(self, key: impl HashKey) -> Option<Value> {
        crate::bridge::raw::hash_lookup(self, key.as_value())
    }
}

/// A Hash key whose lookup runs no Ruby: hashed and compared in C.
pub trait HashKey: ReprValue {}
impl HashKey for Symbol {}
impl HashKey for RString {}

impl RArray {
    /// The elements, read one at a time (`rb_ary_entry`), the length read
    /// afresh before each.
    pub fn each(self) -> impl Iterator<Item = Value> {
        (0..).map_while(move |i: usize| {
            (i < self.len()).then(|| crate::bridge::raw::ary_entry(self, i as c_long))
        })
    }
}

impl Range {
    /// The first end, the last end and whether the last is excluded.
    #[inline]
    pub fn values(self) -> Result<(Value, Value, bool), Error> {
        crate::bridge::raw::range_values(self)
    }
}

impl Integer {
    /// As a `u64`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_u64(self) -> Result<u64, Error> {
        if self.is_negative() {
            return Err(Error::new(
                Ruby::get_with(self).exception_range_error(),
                "can't convert negative integer to unsigned",
            ));
        }
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            return Ok(api.fix2long(self.0 .0) as u64);
        }
        crate::bridge::raw::big2ull(self)
    }

    /// As an `i64`; a value out of its range is Ruby's `RangeError`.
    #[inline]
    pub fn to_i64(self) -> Result<i64, Error> {
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            return Ok(api.fix2long(self.0 .0) as i64);
        }
        crate::bridge::raw::big2ll(self)
    }

    /// As a `usize`; a negative value or one out of range is `RangeError`.
    #[inline]
    pub fn to_usize(self) -> Result<usize, Error> {
        if self.is_negative() {
            return Err(Error::new(
                Ruby::get_with(self).exception_range_error(),
                "can't convert negative integer to unsigned",
            ));
        }
        let api = get_default();
        if api.fixnum_p(self.0 .0) {
            return Ok(api.fix2long(self.0 .0) as usize);
        }
        Ok(crate::bridge::raw::big2ull(self)? as usize)
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
