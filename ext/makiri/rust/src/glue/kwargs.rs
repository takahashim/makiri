//! Reading a method's keyword arguments, kept here so each procedure is written
//! once with its reasons beside it.

use magnus::{prelude::*, Error, RHash, Ruby, Value};

/// The keywords of a method that takes only keywords.
///
/// The no-argument call is `Kwargs(None)`, reached before `scan_args`. It is
/// the overwhelmingly common one - `to_html` with a keyword is the exception -
/// and routing it through `scan_args` cost about a quarter of the per-call
/// throughput on a small element, which is all such a method does at that size.
#[derive(Clone, Copy)]
pub(crate) struct Kwargs(Option<RHash>);

impl Kwargs {
    pub(crate) fn scan(args: &[Value]) -> Result<Kwargs, Error> {
        if args.is_empty() {
            return Ok(Kwargs(None));
        }
        let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
        Ok(Kwargs(Some(scanned.keywords)))
    }

    /// The keyword hash `scan_args` already separated out.
    pub(crate) fn from_hash(h: RHash) -> Kwargs {
        Kwargs(Some(h))
    }

    /// Keyword `name`, or `None` when it is absent or `nil`. A plain lookup
    /// rather than `get_kwargs`, which allocates a second hash for the keys it
    /// was not asked about - so unknown keywords are ignored.
    pub(crate) fn value(self, ruby: &Ruby, name: &str) -> Option<Value> {
        self.0
            .and_then(|h| h.get(ruby.sym_new(name)))
            .filter(|v: &Value| !v.is_nil())
    }

    /// Keyword `name`, read for truthiness: absent and `nil` are false, any
    /// other value - `0` included, this being Ruby - true.
    pub(crate) fn flag(self, ruby: &Ruby, name: &str) -> bool {
        self.value(ruby, name).is_some_and(|v| v.to_bool())
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
        let sym = ruby.sym_new(name);
        let taken = h.get(sym).filter(|v: &Value| !v.is_nil());
        /* Made on the first key that stays, so a call passing only `name`
         * allocates no Hash either. */
        let mut rest: Option<RHash> = None;
        crate::glue::hash::hash_foreach(h, |k, v| {
            if !crate::bridge::ruby::same_value(k, sym.as_value()) {
                rest.get_or_insert_with(|| ruby.hash_new()).aset(k, v)?;
            }
            Ok(())
        })?;
        Ok((taken, rest))
    }
}

/// The `max_tree_depth:` of an HTML parse, as Nokogiri::HTML5 reads it: absent
/// or `nil` is the default (400), a negative Integer means no limit, and any
/// other Integer is the deepest element accepted - a Bignum included, which is
/// simply no limit in practice.
///
/// An actual Integer, not merely something convertible: a Float (1.5) or a
/// String is a `TypeError` rather than a silent truncation or a limit nobody
/// asked for.
pub(crate) fn max_tree_depth(
    ruby: &Ruby,
    v: Option<Value>,
) -> Result<crate::lexbor::adapter::tree_guard::DepthLimit, Error> {
    use crate::lexbor::adapter::tree_guard::DepthLimit;
    let Some(v) = v.filter(|v| !v.is_nil()) else {
        return Ok(DepthLimit::DEFAULT);
    };
    let n = magnus::Integer::from_value(v).ok_or_else(|| {
        Error::new(
            ruby.exception_type_error(),
            "max_tree_depth must be an Integer",
        )
    })?;
    /* Compared before any conversion, so a negative Bignum disables the limit
     * as a negative Fixnum does, and a positive one cannot wrap. */
    if n < ruby.integer_from_i64(0) {
        return Ok(DepthLimit::UNLIMITED);
    }
    Ok(n.to_usize()
        .map_or(DepthLimit::UNLIMITED, DepthLimit::at_most))
}
