//! Reading a Hash argument's pairs.

use magnus::{Error, RArray, RHash, Ruby, Value};

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
