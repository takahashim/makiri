//! Reading a method's keyword arguments, and the pairs of a Hash argument.
//!
//! Two small procedures more than one method needs, kept here so each is
//! written once with its reasons beside it.

use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};

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
}

/// `h`'s key-value pairs, copied out before any of them is used.
///
/// Read from the Hash itself, not through a `to_a` a subclass can redefine (a
/// non-pair tripped an `expect`), and handed out only after the walk, because a
/// caller converts them with Ruby code - `to_s` and the like: inside `foreach`
/// a panic in it became `fatal` (the walk runs under magnus's own `protect`)
/// and a `to_s` that added a key to the same Hash raised "can't add a new key
/// into hash during iteration". The copy runs no Ruby code of the caller's; the
/// Array holding it is a Ruby object on this frame, so the GC sees it.
pub(crate) fn each_pair(
    ruby: &Ruby,
    h: RHash,
    mut f: impl FnMut(Value, Value) -> Result<(), Error>,
) -> Result<(), Error> {
    let pairs: RArray = ruby.ary_new_capa(h.len() * 2);
    h.foreach(|k: Value, v: Value| {
        pairs.push(k)?;
        pairs.push(v)?;
        Ok(magnus::r_hash::ForEach::Continue)
    })?;
    for i in (0..pairs.len()).step_by(2) {
        f(pairs.entry(i as isize)?, pairs.entry(i as isize + 1)?)?;
    }
    Ok(())
}
