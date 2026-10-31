//! `Buf`: the owned, growable byte buffer with a content ceiling that output
//! is collected into, and `OwnedBuf`, the bytes detached from one or copied
//! on their own.
//!
//! A module of its own rather than inside one subsystem because several
//! write into it - the HTML and XML serializers and the XPath engine's string
//! values - and the extension borrows `OwnedBuf` for the copies it must make
//! fallibly (an encoding name, a parse's source, an XML decode). Named by the
//! ceiling, which is what sets it apart from a `Vec`; it was `cbuf` while it
//! was C's libc memory.
//!
//! A `Vec<u8>` grown through `falloc`, so `rake oom` reaches every growth and
//! a failure leaves the buffer as it was. It used to be libc memory under a
//! raw pointer, NUL-terminated for the C callers it once had; nothing reads a
//! terminator any more - every reader takes [`Buf::as_slice`] - so there is
//! none, and no `unsafe` either.
//!
//! Verus proves `Buf` at every length (`verus/ceiling_buf.rs`, its copy held to this
//! file by `rake verus:sync`): the content is what was appended, the room
//! never passes the ceiling - which `append`'s fast path rests on - and a
//! failure changes nothing, a `Limit` exactly when the content would pass
//! the ceiling. `grow_to` tests each falloc result rather than `map_err`ing
//! it, and `append_growing` copies through `as_mut_slice()`, because those
//! are what Verus's std specs can follow; both compile to the same code.

#![forbid(unsafe_code)]

use falloc::{OomOption, Reserve, VecGrow};

/// Why a buffer operation failed. Every failure leaves the buffer as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufError {
    /// Allocation failed, or a size computation overflowed.
    Oom,
    /// The content would pass the buffer's ceiling.
    Limit,
}

impl falloc::Oom for BufError {
    #[inline]
    fn oom() -> Self {
        BufError::Oom
    }
}

/// A growable buffer with a content ceiling.
pub struct Buf {
    /// The room: initialised over its whole length, which is what was asked
    /// for (never past the ceiling), so a short write is two fixed-size
    /// copies into it rather than a `memcpy` call - see [`copy_small`].
    data: Vec<u8>,
    /// How much of `data` is content.
    len: usize,
    /// 0 selects the conservative default ceiling; it is not "unbounded".
    max: usize,
}

/// Bytes detached from a [`Buf`], or copied or filled on their own.
pub struct OwnedBuf(Vec<u8>);

impl OwnedBuf {
    /// The bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// A fresh allocation holding a copy of `bytes`, or None on OOM. The empty
    /// copy allocates nothing.
    pub fn copy_from(bytes: &[u8]) -> Option<OwnedBuf> {
        let mut v = Vec::new();
        v.falloc_reserve_exact(bytes.len()).ok()?;
        v.falloc_extend_from_slice(bytes).ok()?;
        Some(OwnedBuf(v))
    }

    /// Room for exactly `cap` bytes, reserved and EMPTY, that `fill` appends
    /// to without growing it (a Ruby String's `append_to`, which refuses
    /// rather than grows). None on OOM, or when `fill` reports it could not.
    /// Unlike [`OwnedBuf::fill`], nothing is zeroed first: a document's
    /// source is copied once, not written twice.
    pub fn reserved(cap: usize, fill: impl FnOnce(&mut Vec<u8>) -> bool) -> Option<OwnedBuf> {
        let mut v = Vec::new();
        v.falloc_reserve_exact(cap).ok()?;
        fill(&mut v).then_some(OwnedBuf(v))
    }

    /// Room for `cap` bytes, zeroed, that `fill` writes and reports how many it
    /// used. None on OOM. The allocation is owned before `fill` runs, so a
    /// panic in it - or the length check after - releases it.
    pub fn fill(cap: usize, fill: impl FnOnce(&mut [u8]) -> usize) -> Option<OwnedBuf> {
        let mut v = Vec::new();
        v.falloc_reserve_exact(cap).ok()?;
        v.falloc_resize(cap, 0).ok()?;
        let len = fill(&mut v);
        assert!(len <= cap, "OwnedBuf::fill: wrote past its reservation");
        v.truncate(len);
        Some(OwnedBuf(v))
    }
}

impl Buf {
    /// An empty buffer with the soft ceiling `max` (0 = [`BUF_DEFAULT_LIMIT`]).
    ///
    /// The value is not clamped here: every growth path goes through
    /// `content_limit`, which takes `min(max, BUF_HARD_MAX)`, so passing a
    /// larger soft ceiling is already equivalent to passing the hard one.
    pub const fn new(max: usize) -> Buf {
        Buf {
            data: Vec::new(),
            len: 0,
            max,
        }
    }

    /// The bytes written so far.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }

    /// Append `bytes`. Fails closed, leaving the buffer untouched (see
    /// [`BufError`]).
    #[inline]
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), BufError> {
        let n = bytes.len();
        let len = self.len;
        /* The common case - serializer output arrives as many small chunks -
         * is a copy into room already there. `n` fits the room, so `len + n`
         * cannot overflow; and the room is never past the ceiling (`grow_to`,
         * the one place it changes, is handed a clamped size, and
         * verus/ceiling_buf.rs proves it), so content that fits it is within the ceiling. */
        if let Some(dst) = self.data.get_mut(len..len + n) {
            copy_small(dst, bytes);
            self.len = len + n;
            return Ok(());
        }
        self.append_growing(bytes)
    }

    /// [`append`](Self::append) when the bytes do not fit the allocation, or
    /// pass the ceiling: the ceiling, the growth and its clamp. Kept out of
    /// line so the common path stays small enough to inline into a per-chunk
    /// callback.
    #[cold]
    #[inline(never)]
    fn append_growing(&mut self, bytes: &[u8]) -> Result<(), BufError> {
        let need = self.len.checked_add(bytes.len()).or_oom()?;
        let limit = self.content_limit();
        if need > limit {
            return Err(BufError::Limit);
        }
        if need > self.data.len() {
            /* Geometric growth can overshoot to ~2x `need`; it is clamped to
             * the ceiling, which `need` is within, so the clamp never cuts
             * below it. */
            let room = falloc::grow_capacity(self.data.len().max(FIRST_ROOM), need, 1)
                .or_oom()?
                .min(limit);
            self.grow_to(room)?;
        }
        self.data.as_mut_slice()[self.len..need].copy_from_slice(bytes);
        self.len = need;
        Ok(())
    }

    /// Raise the soft ceiling to `max`; a `max` at or below the current one is
    /// ignored. Only ever raising keeps the room within the ceiling, which
    /// [`append`](Self::append)'s fast path relies on.
    pub fn raise_limit(&mut self, max: usize) {
        if max > self.content_limit() {
            self.max = max;
        }
    }

    /// Pre-allocate capacity for `n` content bytes without changing the current
    /// length, so a known-size fill does not reallocate on every geometric step.
    ///
    /// Best-effort: it never grows past the buffer's own ceiling, and a later
    /// append still fails closed if the real output exceeds it.
    pub fn reserve(&mut self, n: usize) -> Result<(), BufError> {
        let n = n.min(self.content_limit());
        if n <= self.data.len() {
            return Ok(()); /* already have room */
        }
        self.grow_to(n)
    }

    /// Detach the bytes and reset this buffer to empty. Moves the allocation;
    /// copies nothing.
    pub fn steal(&mut self) -> OwnedBuf {
        self.data.truncate(self.len);
        self.len = 0;
        OwnedBuf(core::mem::take(&mut self.data))
    }

    /// The effective content ceiling: this buffer's own `max` (0 meaning the
    /// default), clamped by the absolute hard maximum.
    ///
    /// One function, because `append` and `reserve` must not drift - a
    /// `reserve` with a larger ceiling than `append` would pre-size past what
    /// any append will accept.
    #[inline]
    fn content_limit(&self) -> usize {
        Buf::content_limit_for(self.max)
    }

    /// The content ceiling a buffer made by [`Buf::new`]`(max)` enforces: what
    /// a caller that skips the buffer (a value it only borrows) holds the
    /// same bytes to, so both answers fail at the same length.
    #[inline]
    pub fn content_limit_for(max: usize) -> usize {
        let soft = if max != 0 { max } else { BUF_DEFAULT_LIMIT };
        soft.min(BUF_HARD_MAX)
    }

    /// Grow the room to `room` bytes, zeroed: more than it is, and within the
    /// ceiling, which the callers clamp it to. The one place the room changes.
    /// `Oom` leaves the buffer as it was: `falloc`'s reserve is std's
    /// `try_reserve_exact`, which does, and the zeroing after it cannot
    /// allocate.
    fn grow_to(&mut self, room: usize) -> Result<(), BufError> {
        if self
            .data
            .falloc_reserve_exact(room - self.data.len())
            .is_err()
        {
            return Err(BufError::Oom);
        }
        if self.data.falloc_resize(room, 0).is_err() {
            return Err(BufError::Oom);
        }
        Ok(())
    }
}

/// The least room a buffer grows to. Most buffers start empty - a subtree's
/// `to_html`, an XPath string - and grow a few chunks at a time, and each
/// growth is a reallocation plus the zeroing of the new room: starting at 8
/// bytes, a ~150-byte `<li>` took six of them and was ~16% slower than the
/// libc buffer this replaced; starting here it is ~25% faster.
const FIRST_ROOM: usize = 64;

/// Copy `src` into `dst` (the same length), without a `memcpy` call when it
/// is at most 16 elements. Generic only so that `nupur`'s copy of it (over
/// `MaybeUninit<u8>`, where a String is written in place) can be this one
/// token for token, which `rake verus:sync` holds both to.
///
/// Serializer output is mostly chunks of a few bytes - `<`, a tag name, `="` -
/// and a call per chunk into libc's `memcpy` measured about a fifth of
/// `to_html`. Up to 16 bytes are moved as two fixed-size copies that may
/// overlap in the middle (`[0, k)` and `[n - k, n)` for the largest power of
/// two `k <= n`), which covers every byte; each is a load and a store.
#[inline(always)]
pub(crate) fn copy_small<T: Copy>(dst: &mut [T], src: &[T]) {
    #[inline(always)]
    fn two<const K: usize, T: Copy>(dst: &mut [T], src: &[T]) {
        let n = src.len();
        dst[..K].copy_from_slice(&src[..K]);
        dst[n - K..n].copy_from_slice(&src[n - K..]);
    }
    match src.len() {
        0 => {}
        1 => dst[0] = src[0],
        2..=3 => two::<2, T>(dst, src),
        4..=7 => two::<4, T>(dst, src),
        8..=16 => two::<8, T>(dst, src),
        _ => dst.copy_from_slice(src),
    }
}

/* The build-time content limits, each overridable from the environment at
 * build time (`MKR_BUF_HARD_MAX=<bytes>`, `MKR_BUF_DEFAULT_LIMIT=<bytes>`; the
 * variable names predate the port and are kept so an existing build setting
 * still applies). Read in one place, `Buf::content_limit`. */

mod limits {
    use crate::env_bounds::parse_usize;

    /// The absolute ceiling on a buffer's CONTENT length.
    pub(crate) const BUF_HARD_MAX: usize = match option_env!("MKR_BUF_HARD_MAX") {
        Some(s) => parse_usize(s),
        None => 4 << 30, /* 4 GiB */
    };

    /// The ceiling applied when a buffer was initialised with max == 0. Not
    /// "unbounded" - that is the whole point of having a default.
    pub(crate) const BUF_DEFAULT_LIMIT: usize = match option_env!("MKR_BUF_DEFAULT_LIMIT") {
        Some(s) => parse_usize(s),
        None => 100 << 20, /* 100 MiB */
    };
}

pub(crate) use limits::{BUF_DEFAULT_LIMIT, BUF_HARD_MAX};

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::{Buf, BufError, OwnedBuf};

    #[test]
    fn fill_unwinds_on_callback_panic_or_invalid_length() {
        assert!(std::panic::catch_unwind(|| {
            OwnedBuf::fill(32, |_| panic!("fill failed"));
        })
        .is_err());
        assert!(std::panic::catch_unwind(|| OwnedBuf::fill(32, |_| 33)).is_err());

        let owned = OwnedBuf::fill(32, |bytes| {
            assert!(bytes.iter().all(|&b| b == 0));
            bytes[..3].copy_from_slice(b"abc");
            3
        })
        .unwrap();
        assert_eq!(owned.as_slice(), b"abc");
    }

    /// One append, checked against the shadow model as the Kani proof this
    /// replaced checked it: success advances the length by `src.len()`
    /// within the ceiling and the clamped room, failure is the ceiling's and
    /// changes nothing, and the content is the shadow's either way.
    fn step_append(b: &mut Buf, shadow: &mut Vec<u8>, src: &[u8], max: usize) {
        let (len0, cap0) = (b.len, b.data.len());
        match b.append(src) {
            Ok(()) => {
                assert_eq!(b.len, len0 + src.len(), "append: len advances by n");
                assert!(b.len <= max, "append: ceiling respected");
                assert!(b.data.len() <= max, "append: growth clamped to the ceiling");
                shadow.extend_from_slice(src);
            }
            Err(e) => {
                assert_eq!(e, BufError::Limit, "append: the failure reachable here");
                assert!(
                    len0 + src.len() > max,
                    "append: LIMIT only past the ceiling"
                );
                assert_eq!(
                    (b.len, b.data.len()),
                    (len0, cap0),
                    "append: failure leaves the size"
                );
            }
        }
        assert_eq!(
            b.as_slice(),
            &shadow[..],
            "append: content matches the shadow"
        );
    }

    /// Every case of two appends with a reserve between them, over every
    /// ceiling up to 6, append of up to 4 bytes and reserve of up to 8: the
    /// bytes survive every growth, the ceiling and the growth clamp hold,
    /// failures change nothing, and `steal` hands back exactly what went in.
    ///
    /// This was a Kani proof until it ran CBMC out of memory over the `Vec`.
    /// The domain is small enough to try whole, and the bytes
    /// are distinct per position, so a copy that misplaced or dropped one
    /// would show: the code under test never branches on a byte's value,
    /// which is all the proof's symbolic bytes added. Verus now proves the
    /// same at every length (`verus/ceiling_buf.rs`); this stays as the test that
    /// runs the real `Vec`.
    #[test]
    fn append_matches_a_shadow_model_over_the_whole_small_domain() {
        const { assert!(super::BUF_HARD_MAX >= 6 && super::BUF_DEFAULT_LIMIT >= 6) };
        let first: Vec<u8> = (1..=4).collect();
        let second: Vec<u8> = (101..=104).collect();
        let mut cases = 0usize;
        for max in 1..=6 {
            for n1 in 0..=4 {
                for want in 0..=8 {
                    for n2 in 0..=4 {
                        let mut b = Buf::new(max);
                        let mut shadow = Vec::new();
                        step_append(&mut b, &mut shadow, &first[..n1], max);

                        let (len0, cap0) = (b.len, b.data.len());
                        assert_eq!(b.reserve(want), Ok(()), "reserve: no OOM outside rake oom");
                        assert_eq!(b.len, len0, "reserve: len untouched");
                        assert!(b.data.len() <= max, "reserve: clamped to the ceiling");
                        assert!(b.data.len() >= cap0, "reserve: does not shrink the room");
                        assert_eq!(b.as_slice(), &shadow[..], "reserve: content intact");

                        step_append(&mut b, &mut shadow, &second[..n2], max);

                        let owned = b.steal();
                        assert_eq!(owned.as_slice(), &shadow[..], "steal: what was appended");
                        assert!(b.len == 0 && b.data.is_empty(), "steal: buffer reset");
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 6 * 5 * 9 * 5);
    }

    #[test]
    fn safe_api_appends_and_reserves_without_exposing_storage() {
        let mut buf = Buf::new(16);
        assert_eq!(buf.reserve(8), Ok(()));
        assert_eq!(buf.append(b"hello"), Ok(()));
        assert_eq!(buf.as_slice(), b"hello");
        assert_eq!(buf.append(b" world"), Ok(()));
        assert_eq!(buf.as_slice(), b"hello world");
    }

    #[test]
    fn limit_failure_leaves_the_buffer_unchanged() {
        let mut buf = Buf::new(5);
        assert_eq!(buf.append(b"hello"), Ok(()));
        assert_eq!(buf.append(b"!"), Err(BufError::Limit));
        assert_eq!(buf.as_slice(), b"hello");
    }

    /// Room past the ceiling - which a `Vec` may have, however it was asked -
    /// is not a way past it: the fast path checks the ceiling too.
    #[test]
    fn spare_room_does_not_lift_the_ceiling() {
        let mut buf = Buf::new(5);
        assert_eq!(buf.reserve(64), Ok(()));
        assert_eq!(buf.append(b"hello"), Ok(()));
        assert_eq!(buf.append(b"!"), Err(BufError::Limit));
        assert_eq!(buf.as_slice(), b"hello");
    }

    #[test]
    fn raise_limit_only_ever_raises() {
        let mut buf = Buf::new(5);
        assert_eq!(buf.append(b"hello"), Ok(()));
        buf.raise_limit(3); /* a lower ceiling is ignored */
        assert_eq!(buf.append(b"!"), Err(BufError::Limit));
        buf.raise_limit(6);
        assert_eq!(buf.append(b"!"), Ok(()));
        assert_eq!(buf.as_slice(), b"hello!");
    }

    #[test]
    fn steal_transfers_ownership_and_resets_the_buffer() {
        let mut buf = Buf::new(16);
        buf.append(b"owned").unwrap();

        let owned = buf.steal();
        assert_eq!(owned.as_slice(), b"owned");
        assert!(buf.as_slice().is_empty());
        /* The buffer is usable again. */
        buf.append(b"again").unwrap();
        assert_eq!(buf.as_slice(), b"again");
        assert_eq!(owned.as_slice(), b"owned");
    }

    /// Every length the fixed-size path takes, and past it, at every offset
    /// in a buffer that already holds bytes on both sides.
    #[test]
    fn short_copies_move_exactly_their_bytes() {
        for n in 0..=20usize {
            let src: Vec<u8> = (0..n as u8).map(|b| b + 1).collect();
            let mut dst = [0xA5u8; 24];
            super::copy_small(&mut dst[2..2 + n], &src);
            assert_eq!(&dst[2..2 + n], &src[..]);
            assert!(dst[..2].iter().chain(&dst[2 + n..]).all(|&b| b == 0xA5));
        }
    }

    #[test]
    fn interior_nul_is_content() {
        let mut buf = Buf::new(16);
        buf.append(b"a\0b").unwrap();
        assert_eq!(buf.steal().as_slice(), b"a\0b");
    }
}
