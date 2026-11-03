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

use nupur_sys::non_raising;
use nupur_sys::ruby_value_type;

use super::error::Error;
use super::object::{class_inherits, obj_is_kind_of};

// What a String's header says of it: its coderange, encoding index, class.
mod header;
pub(crate) use header::Coderange;
// An Integer as a Rust number or as decimal digits.
mod integer;
// Interned names: a method's, a constant's, a hidden ivar's.
mod id;
pub use id::{HiddenId, Id};

/// Any Ruby value.
#[derive(Clone, Copy)]
#[repr(transparent)]
///
/// `nupur_sys`'s [`nupur_sys::value::Value`] underneath, which is the proof
/// of the GVL its calls take: a value exists only where Ruby runs.
pub struct Value(pub(crate) nupur_sys::value::Value);

impl Value {
    /// The value a `nupur_sys` call returned, which is one already.
    #[inline(always)]
    pub(crate) const fn from_sys(v: nupur_sys::value::Value) -> Value {
        Value(v)
    }
}

/// `Object`, the default superclass (`rb_cObject`, a C global Ruby sets once
/// at boot), for a definition that names none; `of` is the proof of the GVL.
/// `Ruby::class_object` answers the same, for a caller holding the token:
/// this is for `Module::define_data_class`, whose callers hand it none.
#[inline]
pub(crate) fn object_class<V: ReprValue>(of: &V) -> RClass {
    RClass::from_known(Value::from_sys(non_raising::rb_cObject(
        of.as_value().0.ruby(),
    )))
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
        self.as_value().0.is_nil()
    }

    /// Ruby's truthiness: anything but `nil` and `false`.
    #[inline]
    fn to_bool(self) -> bool {
        non_raising::rb_test(self.as_value().0)
    }

    /// Frozen? An immediate (a Fixnum, a Symbol, `nil`) is always frozen.
    #[inline]
    fn is_frozen(self) -> bool {
        let v = self.as_value().0;
        // SAFETY: `v` is a live value; a non-immediate has a header to read.
        non_raising::special_const_p(v) || unsafe { non_raising::frozen_p(v) }
    }

    /// Whether this value's class is `class` itself, as its header holds
    /// it - what `RBASIC_CLASS(v) == class` reads in C. An instance of a
    /// subclass is not, nor one with a singleton class (a `def s.to_json`
    /// makes one), nor an immediate (a Fixnum, a Symbol, a flonum, `nil`),
    /// which has no header. No method is called.
    #[inline]
    fn has_class(self, class: RClass) -> bool {
        let v = self.as_value().0;
        // SAFETY: `v` is a live value; a non-immediate has a header to read.
        !non_raising::special_const_p(v)
            && unsafe { non_raising::rbasic_klass(v) }.is(class.as_value().0)
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

/// `nupur_sys`'s value underneath a typed one, for its calls and nothing
/// else.
pub(crate) trait AsSys {
    /// `nupur_sys`'s value, for its calls: the proof of the GVL they take.
    #[allow(clippy::wrong_self_convention, reason = "a Copy value, read by value")]
    fn as_sys(self) -> nupur_sys::value::Value;
}

impl<T: ReprValue> AsSys for T {
    #[inline]
    fn as_sys(self) -> nupur_sys::value::Value {
        self.as_value().0
    }
}

/// The value of a type `rb_type` names.
#[inline]
fn of_type(v: Value, t: ruby_value_type) -> bool {
    non_raising::rb_type(v.0) == t
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
    |v| non_raising::symbol_p(v.0)
);
handle!(
    /// An Integer: a Fixnum or a Bignum.
    Integer,
    |v| non_raising::integer_type_p(v.0)
);
handle!(
    /// A Float.
    Float,
    |v| non_raising::float_type_p(v.0)
);
handle!(
    /// A Range (or an instance of a subclass).
    Range,
    |v| v.is_kind_of(RClass::from_known(Value::from_sys(non_raising::rb_cRange(v.0.ruby()))))
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
        && class_inherits(v, Value::from_sys(non_raising::rb_eException(v.0.ruby())))
);

impl RString {
    /// The length in bytes.
    #[inline]
    pub fn len(self) -> usize {
        // SAFETY: `self` is a live String.
        unsafe { non_raising::rstring_len(self.0 .0) }
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
        // SAFETY: `self` is a live String; the caller's contract.
        unsafe {
            let (p, n) = (
                non_raising::rstring_ptr(self.0 .0),
                non_raising::rstring_len(self.0 .0),
            );
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
        unsafe { non_raising::rarray_len(self.0 .0) }
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
        unsafe { non_raising::rhash_size(self.0 .0) }
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

impl Symbol {
    /// The Symbol's name, as Ruby holds it (`rb_sym2str`): a frozen String
    /// in the Symbol's own encoding.
    #[inline]
    pub fn name(self) -> Result<RString, Error> {
        crate::raw::sym2str(self)
    }
}

impl Float {
    /// As an `f64`.
    #[inline]
    pub fn to_f64(self) -> f64 {
        // SAFETY: `self` is a live Float.
        unsafe { non_raising::num2dbl(self.0 .0) }
    }
}

/* ---- a value's kind, read once ---- */

/// What a value is, for a reader that dispatches on it: one read of its
/// type (`rb_type`), and the handle for it, rather than a type test per
/// kind. A Bignum and a heap Float are told from a Fixnum and a flonum by
/// [`Kind::Bignum`] and [`Kind::Float`] alike - both are the handle.
#[derive(Clone, Copy)]
pub enum Kind {
    Nil,
    True,
    False,
    /// An Integer that is an immediate.
    Fixnum(Integer),
    /// An Integer on the heap.
    Bignum(Integer),
    Float(Float),
    String(RString),
    Symbol(Symbol),
    Array(RArray),
    Hash(RHash),
    /// Anything else: an object, a class, a Range, a Struct, ...
    Other,
}

impl Value {
    /// This value's [`Kind`]: one read of its type, no call.
    #[inline]
    pub fn kind(self) -> Kind {
        use ruby_value_type as T;
        match non_raising::rb_type(self.0) {
            T::RUBY_T_NIL => Kind::Nil,
            T::RUBY_T_TRUE => Kind::True,
            T::RUBY_T_FALSE => Kind::False,
            T::RUBY_T_FIXNUM => Kind::Fixnum(Integer(self)),
            T::RUBY_T_BIGNUM => Kind::Bignum(Integer(self)),
            T::RUBY_T_FLOAT => Kind::Float(Float(self)),
            T::RUBY_T_STRING => Kind::String(RString(self)),
            T::RUBY_T_SYMBOL => Kind::Symbol(Symbol(self)),
            T::RUBY_T_ARRAY => Kind::Array(RArray(self)),
            T::RUBY_T_HASH => Kind::Hash(RHash(self)),
            _ => Kind::Other,
        }
    }
}

/* ---- immediates and identity ---- */

/// VALUE identity, for the several sites that compare two references.
#[inline]
pub fn same_value(a: impl ReprValue, b: impl ReprValue) -> bool {
    a.as_sys().is(b.as_sys())
}

/* ---- a C read that raises nothing (so it is not `raw`'s, which holds the
 * calls that can) ---- */

/// `rb_ary_entry`: element `i`, `nil` past the end. Raises nothing.
#[inline]
pub(crate) fn ary_entry(a: RArray, i: usize) -> Value {
    /* Converted, not cut: a `long` is 32 bits on Windows, where an index
     * past one would wrap to an element that is there. No Array has one. */
    let Ok(i) = i.try_into() else {
        return Value::from_sys(a.as_sys().ruby().qnil());
    };
    // SAFETY: `a` is a live Array; the read is bounds-checked by Ruby.
    Value::from_sys(unsafe { non_raising::rb_ary_entry(a.as_sys(), i) })
}
