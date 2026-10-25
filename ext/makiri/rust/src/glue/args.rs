//! A method's arguments, read as `rb_scan_args` reads them - in Rust, with no
//! C call that can raise.
//!
//! Ruby 3 passes keywords to a C method as a trailing Hash and says so in the
//! frame (`rb_keyword_given_p`); a Hash passed positionally is positional.
//! So, as `rb_scan_args`'s `:` does:
//!
//! - a method that takes keywords separates the trailing Hash only when the
//!   call gave keywords, and a braced Hash (`f(x, {a: 1})`) stays positional;
//! - a method that takes none receives any keywords as a positional Hash,
//!   which Ruby's own rules make of them.
//!
//! An arity error is `rb_error_arity`'s message, word for word, and counts
//! the positional arguments only. `spec/call_shapes_spec.rb` holds both to
//! what magnus's `scan_args` (over `rb_scan_args`) answered before this
//! replaced it.

use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};

/// What a method takes: `required` positional arguments, then up to
/// `optional` more, then - with `rest` - any number, and keywords when
/// `keywords`.
#[derive(Clone, Copy)]
pub(crate) struct Shape {
    pub required: usize,
    pub optional: usize,
    pub rest: bool,
    pub keywords: bool,
}

impl Shape {
    /// `required` positional arguments and `optional` more.
    pub(crate) const fn positional(required: usize, optional: usize) -> Shape {
        Shape {
            required,
            optional,
            rest: false,
            keywords: false,
        }
    }

    /// The same, and keywords.
    pub(crate) const fn with_keywords(required: usize, optional: usize) -> Shape {
        Shape {
            keywords: true,
            ..Shape::positional(required, optional)
        }
    }
}

/// The arguments of one call, separated.
pub(crate) struct Scanned<'a> {
    positional: &'a [Value],
    keywords: Option<RHash>,
    required: usize,
}

impl<'a> Scanned<'a> {
    /// Required argument `i`.
    pub(crate) fn required(&self, i: usize) -> Value {
        debug_assert!(i < self.required);
        self.positional[i]
    }

    /// Optional argument `i` (counting from the first optional one), or
    /// `None` when it was not passed or was `nil`.
    pub(crate) fn optional(&self, i: usize) -> Option<Value> {
        self.positional
            .get(self.required + i)
            .copied()
            .filter(|v| !v.is_nil())
    }

    /// The arguments past the required ones, for a `rest` shape.
    pub(crate) fn rest(&self) -> &'a [Value] {
        self.positional.get(self.required..).unwrap_or_default()
    }

    /// The keywords, or `None` when the call gave none.
    pub(crate) fn keywords(&self) -> Option<RHash> {
        self.keywords
    }
}

/// Separate `args`, the method's own, by `shape`. Call it first thing in the
/// method's body: whether keywords were given is read from the current frame.
pub(crate) fn scan<'a>(ruby: &Ruby, args: &'a [Value], shape: Shape) -> Result<Scanned<'a>, Error> {
    let (positional, keywords) = match args.split_last() {
        Some((last, init)) if shape.keywords && crate::bridge::rb::method::keyword_given(ruby) => {
            let h = RHash::from_value(*last).ok_or_else(|| {
                crate::bridge::ruby::internal_error("keywords were given, but not as a Hash")
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
pub(crate) fn arity_error(given: usize, min: usize, max: Option<usize>) -> Error {
    let class = crate::bridge::rb::Builtin::ArgumentError;
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
