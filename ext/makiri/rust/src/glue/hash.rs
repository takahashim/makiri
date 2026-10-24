//! Reading a Hash argument's pairs.

use magnus::{Error, RArray, RHash, Ruby, Value};

/// `h`'s pairs, each handed to `f` in turn; the first `Err` stops the walk
/// and is returned.
///
/// Over `bridge::raw::hash_foreach`, which latches a panic in `f` rather than
/// let it unwind into `rb_hash_foreach`'s C frames, and resumes it once they
/// are gone, where `bridge::ruby::entry` makes it `Makiri::InternalError`.
/// `f`'s own error is kept here rather than raised from inside the walk. The
/// one way the glue walks a Hash (`rake unsafe:boundaries` fails on another
/// `.foreach(`).
pub(crate) fn hash_foreach(
    h: RHash,
    mut f: impl FnMut(Value, Value) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut failed: Option<Error> = None;
    crate::bridge::raw::hash_foreach(h, &mut |k, v| match f(k, v) {
        Ok(()) => true,
        Err(e) => {
            failed = Some(e);
            false
        }
    })?;
    failed.map_or(Ok(()), Err)
}

/// `h`'s key-value pairs, copied out before any of them is used.
///
/// Read from the Hash itself, not through a `to_a` a subclass can redefine (a
/// non-pair tripped an `expect`), and handed out only after the walk, so a
/// caller's use of them - which allocates - is outside the iteration. The copy runs no Ruby code of the caller's; the
/// Array holding it is a Ruby object on this frame, so the GC sees it.
pub(crate) fn each_pair(
    _ruby: &Ruby,
    h: RHash,
    mut f: impl FnMut(Value, Value) -> Result<(), Error>,
) -> Result<(), Error> {
    let pairs: RArray = crate::bridge::build::ary(h.len() * 2)?;
    hash_foreach(h, |k, v| {
        crate::bridge::build::push(pairs, k)?;
        crate::bridge::build::push(pairs, v)
    })?;
    for i in (0..pairs.len()).step_by(2) {
        f(pairs.entry(i as isize)?, pairs.entry(i as isize + 1)?)?;
    }
    Ok(())
}
