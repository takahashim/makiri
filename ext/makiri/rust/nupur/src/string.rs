//! A Ruby String's bytes, read and written by copying: the layer hands no
//! slice of a String out.
//!
//! Reading is [`RString::copy_to`] (into a caller's buffer) and
//! [`RString::append_to`] (into a `Vec`'s reserved room, never growing it),
//! each a read and a copy with nothing run between, so nothing Ruby does
//! afterwards - a GC, the String rewritten or grown - reaches what the caller
//! holds. Writing is [`utf8_str_filled`]: a String made at its full length
//! and written front to back in place, through an [`Unwritten`].

#![allow(unsafe_code)]

use crate::rb::value::AsRawValue;
use crate::rb::{Error, RString, Ruby};

/// [`RString::append_to`]'s refusal: the buffer had too little spare room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoRoom;

impl RString {
    /// Copy the bytes from `offset` (none past the end) into `dst`, as many
    /// as both hold, and say how many.
    ///
    /// The String's buffer is read and copied out of in one step, with
    /// nothing run in between, and no slice of it is handed out: a caller
    /// owns what it reads, so nothing Ruby does afterwards - a GC, the String
    /// rewritten or grown - can reach it.
    #[inline]
    pub fn copy_to(self, _ruby: &Ruby, offset: usize, dst: &mut [u8]) -> usize {
        // SAFETY: read and copied out at once; nothing runs while it is held.
        let src = unsafe { self.as_slice() }.get(offset..).unwrap_or(&[]);
        let n = src.len().min(dst.len());
        if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
            d.copy_from_slice(s);
        }
        n
    }

    /// Append the bytes from `offset` (none past the end) to `v`, whose spare
    /// capacity must already hold them - the caller reserved it, fallibly -
    /// and say how many. Nothing is allocated here: when the room is short,
    /// `v` is left as it was and the answer is [`NoRoom`]. The read is
    /// [`copy_to`](Self::copy_to)'s, in one step after the room is checked.
    #[inline]
    pub fn append_to(self, _ruby: &Ruby, offset: usize, v: &mut Vec<u8>) -> Result<usize, NoRoom> {
        // SAFETY: as `copy_to` - read and copied out at once.
        let src = unsafe { self.as_slice() }.get(offset..).unwrap_or(&[]);
        if v.capacity() - v.len() < src.len() {
            return Err(NoRoom);
        }
        #[allow(
            clippy::disallowed_methods,
            reason = "the room was checked above, so it cannot grow"
        )]
        v.extend_from_slice(src);
        Ok(src.len())
    }
}

/// A UTF-8 String of exactly `len` bytes, written by `fill` straight into
/// the new String's buffer - one allocation, no intermediate copy. The
/// String's allocation raises `NoMemoryError`; `fill`'s `Err` is returned in
/// its place, as is a `fill` that wrote fewer than `len` bytes - an internal
/// error, since the bytes left would be whatever the allocation held - and
/// the String is emptied and left to the GC, hidden.
///
/// `fill` writes through an [`Unwritten`], front to back, so every byte the
/// String holds was written and none past its end can be. The String is
/// HIDDEN (`rb_obj_hide`) from its allocation until it is whole, so no Ruby
/// code `fill` runs can find it through `ObjectSpace` - to read the bytes not
/// yet written, or to grow it and move the buffer `fill` writes into - and
/// this frame keeps it alive and in place meanwhile (the GC's stack scan pins
/// it). Shown, as a String, only once it is whole.
///
/// The bytes are tagged UTF-8 without a check; bytes that were not would make
/// a wrong String, never a memory error.
pub fn utf8_str_filled(
    ruby: &Ruby,
    len: usize,
    fill: impl FnOnce(&mut Unwritten<'_>) -> Result<(), Error>,
) -> Result<RString, Error> {
    // SAFETY: filled below, or emptied by `EmptyUnlessFilled` on every other
    // way out, before Ruby code can see it.
    let str = unsafe { crate::raw::utf8_str_unwritten(ruby, len) }?;
    // SAFETY: `str` is the String just made, of `len` bytes (`rstring_ptr`
    // is its buffer, the room `rb_str_new` allocated), referenced only from
    // this frame. Seen as `MaybeUninit`, so nothing reads it unwritten.
    let buf: &mut [core::mem::MaybeUninit<u8>] = unsafe {
        let dst = rb_sys::StableApiDefinition::rstring_ptr(
            rb_sys::stable_api::get_default(),
            str.as_raw(),
        ) as *mut core::mem::MaybeUninit<u8>;
        if len == 0 || dst.is_null() {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(dst, len)
        }
    };
    /* Armed before `fill` runs, so whatever ends it early - an `Err`, or a
     * panic unwinding through this frame to the trampoline - empties the
     * String on the way out; disarmed only once the String is whole. */
    let s = str.as_raw();
    // SAFETY: the String just made, referenced only from this frame; hiding
    // writes only its class field, which the reveal below restores.
    unsafe { rb_sys::rb_obj_hide(s) };
    let unfilled = EmptyUnlessFilled(str);
    let mut w = Unwritten { buf, at: 0 };
    let filled = fill(&mut w).and_then(|()| {
        if w.at == len {
            Ok(())
        } else {
            Err(crate::rb::error::internal_error(
                "a String was left part-written",
            ))
        }
    });
    filled?;
    core::mem::forget(unfilled);
    // SAFETY: the String `rb_obj_hide` hid, whole now, shown as a String.
    unsafe { rb_sys::rb_obj_reveal(s, rb_sys::rb_cString) };
    Ok(str)
}

/// Empties, when dropped, a String [`utf8_str_filled`] has not finished
/// filling: the String is left to the GC, but `ObjectSpace` can still hand it
/// to Ruby code until then, so it must not carry the bytes it was never
/// given - whatever its allocation held before. Empty, it holds none. A
/// `Drop`, so a panic out of `fill` empties it as an `Err` does; forgotten
/// once the String is whole.
struct EmptyUnlessFilled(RString);

impl Drop for EmptyUnlessFilled {
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            ffi_without_gvl_proof,
            reason = "it guards the String `utf8_str_filled` made, with its token, and lives only in that frame"
        )
    )]
    fn drop(&mut self) {
        /* Run while a panic unwinds, too, where a second panic aborts. */
        crate::no_unwind::must_not_unwind::<crate::no_unwind::DropUnfilled, _>(|| {
            let s = self.0.as_raw();
            // SAFETY: the String just made, unfrozen and referenced only
            // from `utf8_str_filled`'s frame; shortening it to nothing
            // raises nothing.
            unsafe { rb_sys::rb_str_set_len(s, 0) };
        })
    }
}

/// The buffer of a String [`utf8_str_filled`] is making, written front to
/// back: the bytes before `at` are written, the rest are not and cannot be
/// read.
pub struct Unwritten<'a> {
    buf: &'a mut [core::mem::MaybeUninit<u8>],
    at: usize,
}

impl Unwritten<'_> {
    /// Append `src`; `false`, with nothing written, when it does not fit.
    #[inline]
    pub fn push(&mut self, src: &[u8]) -> bool {
        let Some(dst) = self
            .at
            .checked_add(src.len())
            .and_then(|end| self.buf.get_mut(self.at..end))
        else {
            return false;
        };
        // SAFETY: `u8` and `MaybeUninit<u8>` have one layout, and every `u8`
        // is a valid `MaybeUninit<u8>`; the view is only read.
        let src: &[core::mem::MaybeUninit<u8>] =
            unsafe { &*(src as *const [u8] as *const [core::mem::MaybeUninit<u8>]) };
        copy_small(dst, src);
        self.at += src.len();
        true
    }

    /// How many bytes are written.
    #[inline]
    pub fn written(&self) -> usize {
        self.at
    }
}

/// `dst.copy_from_slice(src)` for slices of one length, with no `memcpy`
/// call for 16 elements or fewer: two fixed-size copies that overlap. Text
/// slices are mostly a few bytes, and a `memcpy` call each measured
/// `doc.text` ~50% slower. The extension's `cbuf::copy_small`, token for
/// token, which Verus proves (`verus/cbuf.rs`); `rake verus:sync` holds this
/// copy to it too, since this crate cannot depend on the extension.
#[inline(always)]
#[allow(
    clippy::indexing_slicing,
    reason = "dst and src are one length, at least K"
)]
fn copy_small<T: Copy>(dst: &mut [T], src: &[T]) {
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
