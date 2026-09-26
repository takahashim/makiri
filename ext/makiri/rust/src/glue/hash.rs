//! Reading a Hash argument's pairs.

use magnus::{Error, RArray, RHash, Ruby, Value};

use crate::caught::PanicLatch;

/// `rb_hash_foreach` over `h`, with a panic in `f` caught.
///
/// magnus runs the closure inside its own `protect`, below `rb_hash_foreach`'s
/// C frames, so a panic there would unwind INTO C and abort the process. It is
/// latched instead - the walk stops - and raised again once the C frames are
/// gone, where `bridge::ruby::entry` makes it `Makiri::InternalError`. The one
/// way the glue walks a Hash (`rake unsafe:boundaries` fails on another
/// `.foreach(`).
pub(crate) fn hash_foreach(
    h: RHash,
    mut f: impl FnMut(Value, Value) -> Result<(), Error>,
) -> Result<(), Error> {
    use magnus::r_hash::ForEach;
    let mut latch = PanicLatch::new();
    let walked = h.foreach(|k: Value, v: Value| {
        latch.guard(Ok(ForEach::Stop), || f(k, v).map(|()| ForEach::Continue))
    });
    latch.resume();
    walked
}

/// `h`'s key-value pairs, copied out before any of them is used.
///
/// Read from the Hash itself, not through a `to_a` a subclass can redefine (a
/// non-pair tripped an `expect`), and handed out only after the walk, because a
/// caller converts them with Ruby code - `to_s` and the like: inside the walk a
/// `to_s` that added a key to the same Hash raised "can't add a new key into
/// hash during iteration". The copy runs no Ruby code of the caller's; the
/// Array holding it is a Ruby object on this frame, so the GC sees it.
pub(crate) fn each_pair(
    ruby: &Ruby,
    h: RHash,
    mut f: impl FnMut(Value, Value) -> Result<(), Error>,
) -> Result<(), Error> {
    let pairs: RArray = ruby.ary_new_capa(h.len() * 2);
    hash_foreach(h, |k, v| {
        pairs.push(k)?;
        pairs.push(v)
    })?;
    for i in (0..pairs.len()).step_by(2) {
        f(pairs.entry(i as isize)?, pairs.entry(i as isize + 1)?)?;
    }
    Ok(())
}
