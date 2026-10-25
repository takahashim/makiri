//! Reading a method's keyword arguments, kept here so each procedure is written
//! once with its reasons beside it.

use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};

/// The keywords of a method that takes only keywords.
///
/// The no-argument call is `Kwargs(None)`, answered before any scan. It is
/// the overwhelmingly common one - `to_html` with a keyword is the exception -
/// and on a small element the call does little else.
#[derive(Clone, Copy)]
pub(crate) struct Kwargs(Option<RHash>);

impl Kwargs {
    pub(crate) fn scan(ruby: &Ruby, args: &[Value]) -> Result<Kwargs, Error> {
        if args.is_empty() {
            return Ok(Kwargs(None));
        }
        let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::with_keywords(0, 0))?;
        Ok(Kwargs(a.keywords()))
    }

    /// The keywords a scan already separated out (`None` for none).
    pub(crate) fn from_scan(h: Option<RHash>) -> Kwargs {
        Kwargs(h)
    }

    /// Keyword `name`, or `None` when it is absent or `nil`. A plain lookup
    /// rather than `get_kwargs`, which allocates a second hash for the keys it
    /// was not asked about - so unknown keywords are ignored.
    pub(crate) fn value(self, ruby: &Ruby, name: &str) -> Result<Option<Value>, Error> {
        let Some(h) = self.0 else {
            return Ok(None);
        };
        Ok(h.get(crate::bridge::build::symbol(ruby, name)?)?
            .filter(|v: &Value| !v.is_nil()))
    }

    /// Keyword `name`, read for truthiness: absent and `nil` are false, any
    /// other value - `0` included, this being Ruby - true.
    pub(crate) fn flag(self, ruby: &Ruby, name: &str) -> Result<bool, Error> {
        Ok(self.value(ruby, name)?.is_some_and(|v| v.to_bool()))
    }

    /// Keyword `name`, and the OTHER keywords as a fresh Hash (`None` when none
    /// remain).
    ///
    /// The key is dropped by raw identity, which is sound because keyword keys
    /// are interned symbols. Copied rather than mutated, and read through the
    /// Hash storage (`rb_hash_foreach`/`rb_hash_aset`) rather than
    /// `dup`/`delete`, which a Hash subclass can redefine.
    pub(crate) fn split_off(
        self,
        ruby: &Ruby,
        name: &str,
    ) -> Result<(Option<Value>, Option<RHash>), Error> {
        /* No keywords is the common call: answer it without a symbol or a Hash. */
        let Some(h) = self.0.filter(|h| !h.is_empty()) else {
            return Ok((None, None));
        };
        let sym = crate::bridge::build::symbol(ruby, name)?;
        let taken = h.get(sym)?.filter(|v: &Value| !v.is_nil());
        let sym = sym.as_value();
        /* Made on the first key that stays, so a call passing only `name`
         * allocates no Hash either. */
        let mut rest: Option<RHash> = None;
        crate::glue::hash::hash_foreach(h, |k, v| {
            if !crate::bridge::ruby::same_value(k, sym) {
                let into = match rest {
                    Some(h) => h,
                    None => *rest.insert(crate::bridge::build::hash(ruby)?),
                };
                crate::bridge::build::aset(into, k, v)?;
            }
            Ok(())
        })?;
        Ok((taken, rest))
    }
}

/// The limits of an HTML parse from its `max_tree_depth:` and
/// `max_amplification:` keywords ([`max_tree_depth`], [`max_amplification`]).
pub(crate) fn parse_limits(
    ruby: &Ruby,
    depth: Option<Value>,
    amplification: Option<Value>,
) -> Result<crate::html_dom::tree_guard::ParseLimits, Error> {
    use crate::html_dom::tree_guard::ParseLimits;
    let depth = max_tree_depth(ruby, depth)?;
    let ratio = max_amplification(ruby, amplification)?;
    Ok(ParseLimits::DEFAULT
        .with_depth(depth)
        .with_amplification(ratio))
}

/// The `max_amplification:` of an HTML parse: how many times its input a
/// document may build (`html::tree::guard`), never less than 64 MiB. Absent or
/// `nil` is the default (256); a negative Integer means no limit; 0 leaves
/// only the 64 MiB floor. An actual Integer, as [`max_tree_depth`].
fn max_amplification(ruby: &Ruby, v: Option<Value>) -> Result<Option<usize>, Error> {
    let Some(v) = v.filter(|v| !v.is_nil()) else {
        return Ok(crate::html_dom::tree_guard::ParseLimits::DEFAULT.max_amplification());
    };
    let n = crate::bridge::rb::Integer::from_value(v).ok_or_else(|| {
        Error::new(
            crate::bridge::rb::Builtin::TypeError,
            "max_amplification must be an Integer",
        )
    })?;
    if n.is_negative() {
        return Ok(None);
    }
    /* Past a usize is no limit in practice, as for the depth. */
    n.to_usize_within(ruby)
}

/// The `max_tree_depth:` of an HTML parse, as Nokogiri::HTML5 reads it: absent
/// or `nil` is the default (400), a negative Integer means no limit, and any
/// other Integer is the deepest element accepted - a Bignum included, which is
/// simply no limit in practice.
///
/// An actual Integer, not merely something convertible: a Float (1.5) or a
/// String is a `TypeError` rather than a silent truncation or a limit nobody
/// asked for.
fn max_tree_depth(ruby: &Ruby, v: Option<Value>) -> Result<Option<usize>, Error> {
    let Some(v) = v.filter(|v| !v.is_nil()) else {
        return Ok(crate::html_dom::tree_guard::ParseLimits::DEFAULT.max_depth());
    };
    let n = crate::bridge::rb::Integer::from_value(v).ok_or_else(|| {
        Error::new(
            crate::bridge::rb::Builtin::TypeError,
            "max_tree_depth must be an Integer",
        )
    })?;
    /* The sign is read before any conversion, so a negative Bignum disables
     * the limit as a negative Fixnum does, and a positive one cannot wrap. */
    if n.is_negative() {
        return Ok(None);
    }
    n.to_usize_within(ruby)
}
