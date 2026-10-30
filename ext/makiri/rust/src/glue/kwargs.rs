//! The keywords of Makiri's own parse entries. Reading keywords in general
//! is `nupur::args`' (`Kwargs`).

use crate::bridge::rb::{prelude::*, Error, Value};

/// The limits of an HTML parse from its `max_tree_depth:` and
/// `max_amplification:` keywords ([`max_tree_depth`], [`max_amplification`]).
pub(crate) fn parse_limits(
    depth: Option<Value>,
    amplification: Option<Value>,
) -> Result<crate::html_dom::tree_guard::ParseLimits, Error> {
    use crate::html_dom::tree_guard::ParseLimits;
    let depth = max_tree_depth(depth)?;
    let ratio = max_amplification(amplification)?;
    Ok(ParseLimits::DEFAULT
        .with_depth(depth)
        .with_amplification(ratio))
}

/// The `max_amplification:` of an HTML parse: how many times its input a
/// document may build (`html::tree::guard`), never less than 64 MiB. Absent or
/// `nil` is the default (256); a negative Integer means no limit; 0 leaves
/// only the 64 MiB floor. An actual Integer, as [`max_tree_depth`].
fn max_amplification(v: Option<Value>) -> Result<Option<usize>, Error> {
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
    n.to_usize_within()
}

/// The `max_tree_depth:` of an HTML parse, as Nokogiri::HTML5 reads it: absent
/// or `nil` is the default (400), a negative Integer means no limit, and any
/// other Integer is the deepest element accepted - a Bignum included, which is
/// simply no limit in practice.
///
/// An actual Integer, not merely something convertible: a Float (1.5) or a
/// String is a `TypeError` rather than a silent truncation or a limit nobody
/// asked for.
fn max_tree_depth(v: Option<Value>) -> Result<Option<usize>, Error> {
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
    n.to_usize_within()
}
