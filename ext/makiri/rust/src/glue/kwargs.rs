//! The keywords of Makiri's own parse entries. Reading keywords in general
//! is `nupur::args`' (`Kwargs`).

use crate::bridge::rb::{prelude::*, Error, Value};
use crate::html_dom::tree_guard::ParseLimits;

/// The limits of an HTML parse from its `max_tree_depth:` and
/// `max_amplification:` keywords.
///
/// `max_tree_depth:` is read as Nokogiri::HTML5 reads it: the deepest element
/// accepted (default 400). `max_amplification:` is how many times its input a
/// document may build (`html::tree::guard`, default 256), never less than
/// 64 MiB - so 0 leaves only that floor.
pub(crate) fn parse_limits(
    depth: Option<Value>,
    amplification: Option<Value>,
) -> Result<ParseLimits, Error> {
    let depth = limit_keyword(
        depth,
        "max_tree_depth must be an Integer",
        ParseLimits::DEFAULT.max_depth(),
    )?;
    let ratio = limit_keyword(
        amplification,
        "max_amplification must be an Integer",
        ParseLimits::DEFAULT.max_amplification(),
    )?;
    Ok(ParseLimits::DEFAULT
        .with_depth(depth)
        .with_amplification(ratio))
}

/// One limit keyword: absent or `nil` is `default`, a negative Integer means
/// no limit, and any other Integer is the limit - a Bignum included, which is
/// simply no limit in practice.
///
/// An actual Integer, not merely something convertible: a Float (1.5) or a
/// String is a `TypeError` (`not_integer`) rather than a silent truncation or
/// a limit nobody asked for.
fn limit_keyword(
    v: Option<Value>,
    not_integer: &'static str,
    default: Option<usize>,
) -> Result<Option<usize>, Error> {
    let Some(v) = v.filter(|v| !v.is_nil()) else {
        return Ok(default);
    };
    let n = crate::bridge::rb::Integer::from_value(v)
        .ok_or_else(|| Error::new(crate::bridge::rb::Builtin::TypeError, not_integer))?;
    /* The sign is read before any conversion, so a negative Bignum disables
     * the limit as a negative Fixnum does, and a positive one cannot wrap. */
    if n.is_negative() {
        return Ok(None);
    }
    /* Past a usize is no limit in practice. */
    n.to_usize_within()
}
