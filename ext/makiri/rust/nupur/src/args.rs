//! Reading a method's arguments: an arity -1 method's `&[Value]` separated
//! into positional arguments and keywords ([`scan`]), keywords read by name
//! ([`Kwargs`]), and each value's type checked with no conversion - a
//! `to_s`, `to_str` or `to_int` is the caller's Ruby code, which the layer
//! runs none of - so a value of another type is a `TypeError`, worded as
//! Ruby's own type check words it, with the way out.

#![forbid(unsafe_code)]

use crate::describe::ClassName;
use crate::rb::{prelude::*, Error, RHash, RString, Ruby, Value};

/// `v`, which must be a String (a subclass's instance included).
///
/// There is no conversion: a `to_str` or `to_s` is the caller's Ruby code, and
/// the layer runs none, so anything else is a `TypeError`, worded as Ruby's own
/// type check words it.
pub fn string_arg(v: Value) -> Result<RString, Error> {
    RString::from_value(v).ok_or_else(|| wrong_type(v, "String", TO_S_HINT))
}

/// `v`, which must be an Integer (a Fixnum or a Bignum). No conversion, as
/// [`string_arg`]: a `to_int` is the caller's Ruby.
pub fn integer_arg(v: Value) -> Result<crate::rb::Integer, Error> {
    crate::rb::Integer::from_value(v).ok_or_else(|| wrong_type(v, "Integer", TO_I_HINT))
}

/// [`integer_arg`] as an index or a count: an `i64` - the same on every
/// target, where a C `long` is 32 bits on Windows - and Ruby's `RangeError`
/// past one.
pub fn index_arg(v: Value) -> Result<i64, Error> {
    integer_arg(v)?.to_i64()
}

/// A Range argument over a collection of `count` elements, as `Array#[]`
/// reads one (`rb_range_beg_len`): the start and the length it selects,
/// `None` for a start outside the collection, `RangeError` for a bound too
/// large, and a `TypeError` for a Range whose ends are not Integers or nil -
/// whose `to_int` the C would run.
pub fn range_beg_len(range: Value, count: usize) -> Result<Option<(usize, usize)>, Error> {
    crate::raw::range_beg_len(range, count)
}

const TO_S_HINT: &str = "; call #to_s to convert it";
const TO_I_HINT: &str = "; call #to_i to convert it";

/// `v` as a String for a NAME - anything a caller may write as a Symbol as
/// well: a String, or a Symbol's name. Nothing else
/// converts, as [`string_arg`].
pub fn name_arg(v: Value) -> Result<RString, Error> {
    if let Some(s) = RString::from_value(v) {
        return Ok(s);
    }
    match crate::rb::Symbol::from_value(v) {
        Some(sym) => super::raw::sym2str(sym),
        None => Err(wrong_type(v, "String or Symbol", TO_S_HINT)),
    }
}

/// Ruby's `TypeError` for a value of the wrong class, worded as Ruby's own
/// type check words it, with the way out (`hint`): "wrong argument type
/// Integer (expected String); call #to_s to convert it". The layer converts
/// nothing itself (a `to_s` is the caller's Ruby), so the caller does. The
/// class name is read without a call.
fn wrong_type(v: Value, expected: &str, hint: &str) -> Error {
    crate::rb::error::type_error(format_args!(
        "wrong argument type {} (expected {expected}){hint}",
        ClassName(v)
    ))
}

/* ---- separating a call's arguments ---- */

/* A method's arguments, read as `rb_scan_args` reads them - in Rust, with no
 * C call that can raise.
 *
 * Ruby 3 passes keywords to a C method as a trailing Hash and says so in the
 * frame (`rb_keyword_given_p`); a Hash passed positionally is positional.
 * So, as `rb_scan_args`'s `:` does:
 *
 * - a method that takes keywords separates the trailing Hash only when the
 *   call gave keywords, and a braced Hash (`f(x, {a: 1})`) stays positional;
 * - a method that takes none receives any keywords as a positional Hash,
 *   which Ruby's own rules make of them.
 *
 * An arity error is `rb_error_arity`'s message, word for word, and counts
 * the positional arguments only. `spec/call_shapes_spec.rb` holds both to
 * what magnus's `scan_args` (over `rb_scan_args`) answered before this
 * replaced it. */

/// What a method takes: `required` positional arguments, then up to
/// `optional` more, then - with `rest` - any number, and keywords when
/// `keywords`.
#[derive(Clone, Copy)]
pub struct Shape {
    pub required: usize,
    pub optional: usize,
    pub rest: bool,
    pub keywords: bool,
}

impl Shape {
    /// `required` positional arguments and `optional` more.
    pub const fn positional(required: usize, optional: usize) -> Shape {
        Shape {
            required,
            optional,
            rest: false,
            keywords: false,
        }
    }

    /// The same, and keywords.
    pub const fn with_keywords(required: usize, optional: usize) -> Shape {
        Shape {
            keywords: true,
            ..Shape::positional(required, optional)
        }
    }
}

/// The arguments of one call, separated.
pub struct Scanned<'a> {
    positional: &'a [Value],
    keywords: Option<RHash>,
    required: usize,
}

impl<'a> Scanned<'a> {
    /// Required argument `i`.
    pub fn required(&self, i: usize) -> Value {
        debug_assert!(i < self.required);
        self.positional[i]
    }

    /// Optional argument `i` (counting from the first optional one), or
    /// `None` when it was not passed or was `nil`.
    pub fn optional(&self, i: usize) -> Option<Value> {
        self.positional
            .get(self.required + i)
            .copied()
            .filter(|v| !v.is_nil())
    }

    /// The arguments past the required ones, for a `rest` shape.
    pub fn rest(&self) -> &'a [Value] {
        self.positional.get(self.required..).unwrap_or_default()
    }

    /// The keywords, or `None` when the call gave none.
    pub fn keywords(&self) -> Option<RHash> {
        self.keywords
    }
}

/// Separate `args`, the method's own, by `shape`. Call it first thing in the
/// method's body: whether keywords were given is read from the current frame.
pub fn scan<'a>(ruby: &Ruby, args: &'a [Value], shape: Shape) -> Result<Scanned<'a>, Error> {
    let (positional, keywords) = match args.split_last() {
        Some((last, init)) if shape.keywords && crate::rb::method::keyword_given(ruby) => {
            let h = RHash::from_value(*last).ok_or_else(|| {
                crate::rb::error::internal_error("keywords were given, but not as a Hash")
            })?;
            (init, Some(h))
        }
        _ => (args, None),
    };
    let max = shape.required.saturating_add(shape.optional);
    let given = positional.len();
    if given < shape.required || (!shape.rest && given > max) {
        return Err(arity_error(
            given,
            shape.required,
            (!shape.rest).then_some(max),
        ));
    }
    Ok(Scanned {
        positional,
        keywords,
        required: shape.required,
    })
}

/// Ruby's `ArgumentError` for `given` arguments where `min` to `max` (`None`:
/// any number) are taken: `rb_error_arity`'s message.
pub fn arity_error(given: usize, min: usize, max: Option<usize>) -> Error {
    let class = crate::rb::Builtin::ArgumentError;
    match max {
        Some(max) if max == min => Error::new(
            class,
            format_args!("wrong number of arguments (given {given}, expected {min})"),
        ),
        Some(max) => Error::new(
            class,
            format_args!("wrong number of arguments (given {given}, expected {min}..{max})"),
        ),
        None => Error::new(
            class,
            format_args!("wrong number of arguments (given {given}, expected {min}+)"),
        ),
    }
}

/* ---- keywords ---- */

/// The keywords of a method that takes only keywords.
///
/// The no-argument call is `Kwargs(None)`, answered before any scan. It is
/// the overwhelmingly common one - `to_html` with a keyword is the exception -
/// and on a small element the call does little else.
#[derive(Clone, Copy)]
pub struct Kwargs(Option<RHash>);

impl Kwargs {
    pub fn scan(ruby: &Ruby, args: &[Value]) -> Result<Kwargs, Error> {
        if args.is_empty() {
            return Ok(Kwargs(None));
        }
        let a = scan(ruby, args, Shape::with_keywords(0, 0))?;
        Ok(Kwargs(a.keywords()))
    }

    /// The keywords a scan already separated out (`None` for none).
    pub fn from_scan(h: Option<RHash>) -> Kwargs {
        Kwargs(h)
    }

    /// Keyword `name`, or `None` when it is absent or `nil`. A plain lookup
    /// rather than `get_kwargs`, which allocates a second hash for the keys it
    /// was not asked about - so unknown keywords are ignored.
    pub fn value(self, ruby: &Ruby, name: &str) -> Result<Option<Value>, Error> {
        let Some(h) = self.0 else {
            return Ok(None);
        };
        Ok(h.get(crate::build::symbol(ruby, name)?)?
            .filter(|v: &Value| !v.is_nil()))
    }

    /// Keyword `name`, read for truthiness: absent and `nil` are false, any
    /// other value - `0` included, this being Ruby - true.
    pub fn flag(self, ruby: &Ruby, name: &str) -> Result<bool, Error> {
        Ok(self.value(ruby, name)?.is_some_and(|v| v.to_bool()))
    }

    /// Keyword `name`, and the OTHER keywords as a fresh Hash (`None` when none
    /// remain).
    ///
    /// The key is dropped by raw identity, which is sound because keyword keys
    /// are interned symbols. Copied rather than mutated, and read through the
    /// Hash storage (`rb_hash_foreach`/`rb_hash_aset`) rather than
    /// `dup`/`delete`, which a Hash subclass can redefine.
    pub fn split_off(
        self,
        ruby: &Ruby,
        name: &str,
    ) -> Result<(Option<Value>, Option<RHash>), Error> {
        /* No keywords is the common call: answer it without a symbol or a Hash. */
        let Some(h) = self.0.filter(|h| !h.is_empty()) else {
            return Ok((None, None));
        };
        let sym = crate::build::symbol(ruby, name)?;
        let taken = h.get(sym)?.filter(|v: &Value| !v.is_nil());
        let sym = sym.as_value();
        /* Made on the first key that stays, so a call passing only `name`
         * allocates no Hash either. */
        let mut rest: Option<RHash> = None;
        crate::hash::hash_foreach(h, |k, v| {
            if !crate::rb::value::same_value(k, sym) {
                let into = match rest {
                    Some(h) => h,
                    None => *rest.insert(crate::build::hash(ruby)?),
                };
                crate::build::aset(into, k, v)?;
            }
            Ok(())
        })?;
        Ok((taken, rest))
    }
}
