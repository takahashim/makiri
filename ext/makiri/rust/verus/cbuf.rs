//! Verus proofs over `src/cbuf.rs`: `Buf`, the capped byte buffer output is
//! collected into, holds exactly what was appended, never past its ceiling,
//! and a failure changes nothing - for every length, where the exhaustive
//! test (`cbuf::tests`) covers ceilings up to 6 bytes and the Kani proof it
//! replaced ran CBMC out of memory.
//!
//! What is proved is the shipped code: `Buf`'s `as_slice`, `append`,
//! `append_growing`, `raise_limit`, `reserve`, `steal`, `content_limit`,
//! `content_limit_for` and `grow_to`, `copy_small`, and falloc's
//! `grow_capacity` are copied, every line only Verus needs ends in
//! `// [verus]`, and `rake verus:sync` checks the rest of each body against
//! the source. The invariant the fast path rests on - the room is never past
//! the ceiling, so content that fits the room fits the ceiling - is `wf`.
//!
//! The axioms: falloc's `falloc_reserve_exact` (changes no content, or
//! fails), `falloc_resize` (the content, then `value` up to `new_len`, or
//! nothing changed) and `or_oom`; std's `mem::take` (the value, and the
//! default left behind); and that a slice of bytes is at most `isize::MAX`
//! long, as every Rust slice is.

use vstd::prelude::*;

verus! {

// Makiri builds for 64-bit targets only.
global size_of usize == 8;

pub mod falloc {
    use vstd::prelude::*;

    /// `need * elem` fits in a `usize`.
    pub open spec fn fits(n: int, elem: int) -> bool {
        0 <= n * elem <= usize::MAX
    }

    pub fn grow_capacity(cap: usize, need: usize, elem: usize) -> (result: Option<usize>)
        ensures // [verus]
            result.is_none() <==> !fits(need as int, elem as int), // [verus]
            need > 0 && result.is_some() ==> result.unwrap() >= need // [verus]
                && fits(result.unwrap() as int, elem as int), // [verus]
            need > 0 && result.is_some() && cap > 0 && fits(cap as int, elem as int) // [verus]
                ==> result.unwrap() >= cap, // [verus]
    {
        need.checked_mul(elem)?;
        if need == 0 {
            // [canary]
            return Some(if cap != 0 && cap.checked_mul(elem).is_some() {
                cap
            } else {
                0
            });
        }
        let start = match cap.checked_mul(elem) {
            Some(_) if cap != 0 => cap,
            _ if 8usize.checked_mul(elem).is_some() => 8,
            _ => need,
        };
        let mut nc = start;
        while nc < need
            invariant // [verus]
                nc >= 1, // [verus]
                fits(nc as int, elem as int), // [verus]
                fits(need as int, elem as int), // [verus]
                need > 0, // [verus]
                cap > 0 && fits(cap as int, elem as int) ==> nc >= cap, // [verus]
            decreases usize::MAX - nc, // [verus]
        {
            // [canary]
            match nc.checked_mul(2) {
                Some(next) if next.checked_mul(elem).is_some() => nc = next,
                _ => return Some(need),
            }
        }
        // [canary]
        Some(nc)
    }

    pub trait Oom: Sized {
        fn oom() -> Self;
    }

    pub trait OomOption<T> {
        fn or_oom<E: Oom>(self) -> (r: Result<T, E>);
    }

    impl<T> OomOption<T> for Option<T> {
        #[verifier::external_body]
        fn or_oom<E: Oom>(self) -> (r: Result<T, E>)
            ensures
                self.is_some() <==> r.is_ok(),
                self.is_some() ==> r == Ok::<T, E>(self.unwrap()),
        {
            unimplemented!()
        }
    }

    pub trait Reserve {
        fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()>;
    }

    impl Reserve for Vec<u8> {
        /// Room for `additional` more, which changes no content - or `Err`.
        #[verifier::external_body]
        fn falloc_reserve_exact(&mut self, additional: usize) -> (r: Result<(), ()>)
            ensures
                final(self)@ == old(self)@,
        {
            unimplemented!()
        }
    }

    pub trait VecGrow<T> {
        fn falloc_resize(&mut self, new_len: usize, value: T) -> Result<(), ()>;
    }

    impl VecGrow<u8> for Vec<u8> {
        /// The content, then `value` up to `new_len` (>= the length here) -
        /// or `Err` with nothing changed.
        #[verifier::external_body]
        fn falloc_resize(&mut self, new_len: usize, value: u8) -> (r: Result<(), ()>)
            ensures
                r.is_ok() ==> final(self)@ == old(self)@ + Seq::new(
                    (new_len - old(self)@.len()) as nat,
                    |_i: int| value,
                ),
                r.is_err() ==> final(self)@ == old(self)@,
        {
            unimplemented!()
        }
    }
}

use falloc::{OomOption, Reserve, VecGrow};

pub assume_specification<T: Default>[ core::mem::take::<T> ](dest: &mut T) -> (r: T)
    ensures
        r == *old(dest),
        call_ensures(T::default, (), *final(dest)),
;

#[derive(PartialEq, Eq)]
pub enum BufError {
    Oom,
    Limit,
}

impl falloc::Oom for BufError {
    fn oom() -> Self {
        BufError::Oom
    }
}

pub const BUF_HARD_MAX: usize = 4 << 30;
pub const BUF_DEFAULT_LIMIT: usize = 100 << 20;
pub const FIRST_ROOM: usize = 64;

proof fn lemma_limits()
    ensures
        BUF_HARD_MAX == 0x1_0000_0000usize,
        BUF_DEFAULT_LIMIT == 0x640_0000usize,
{
    assert(4usize << 30usize == 0x1_0000_0000usize) by (bit_vector);
    assert(100usize << 20usize == 0x640_0000usize) by (bit_vector);
}

/// The ceiling `Buf::content_limit_for(max)` answers.
pub open spec fn limit_for(max: usize) -> int {
    let soft = if max != 0 { max as int } else { BUF_DEFAULT_LIMIT as int };
    if soft < BUF_HARD_MAX { soft } else { BUF_HARD_MAX as int }
}

/// The fields are the source's; public here so the specs below can name them.
pub struct Buf {
    pub data: Vec<u8>,
    pub len: usize,
    pub max: usize,
}

pub struct OwnedBuf(pub Vec<u8>);

impl Buf {
    /// The invariant: the content is the first `len` bytes of the room, and
    /// the room is never past the ceiling.
    pub open spec fn wf(&self) -> bool {
        self.len <= self.data@.len() <= limit_for(self.max)
    }

    /// The content.
    pub open spec fn view(&self) -> Seq<u8> {
        self.data@.subrange(0, self.len as int)
    }

    pub fn as_slice(&self) -> (s: &[u8])
        requires // [verus]
            self.wf(), // [verus]
        ensures // [verus]
            s@ == self.view(), // [verus]
    {
        // [canary]
        &self.data[..self.len]
    }

    pub fn append(&mut self, bytes: &[u8]) -> (r: Result<(), BufError>)
        requires // [verus]
            old(self).wf(), // [verus]
            bytes@.len() <= isize::MAX, // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).max == old(self).max, // [verus]
            r.is_ok() ==> final(self).view() == old(self).view() + bytes@, // [verus]
            r.is_err() ==> final(self).view() == old(self).view(), // [verus]
            r == Err::<(), BufError>(BufError::Limit) // [verus]
                <==> old(self).len + bytes@.len() > limit_for(old(self).max), // [verus]
    {
        proof { lemma_limits(); } // [verus]
        let n = bytes.len();
        let len = self.len;
        if let Some(dst) = self.data.get_mut(len..len + n) {
            // [canary]
            copy_small(dst, bytes);
            self.len = len + n;
            assert(self.view() =~= old(self).view() + bytes@); // [verus]
            return Ok(());
        }
        // [canary]
        self.append_growing(bytes)
    }

    fn append_growing(&mut self, bytes: &[u8]) -> (r: Result<(), BufError>)
        requires // [verus]
            old(self).wf(), // [verus]
            old(self).len + bytes@.len() > old(self).data@.len(), // [verus]
            bytes@.len() <= isize::MAX, // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).max == old(self).max, // [verus]
            r.is_ok() ==> final(self).view() == old(self).view() + bytes@, // [verus]
            r.is_err() ==> final(self).view() == old(self).view(), // [verus]
            r == Err::<(), BufError>(BufError::Limit) // [verus]
                <==> old(self).len + bytes@.len() > limit_for(old(self).max), // [verus]
    {
        proof { lemma_limits(); } // [verus]
        let need = self.len.checked_add(bytes.len()).or_oom()?;
        let limit = self.content_limit();
        if need > limit {
            // [canary]
            return Err(BufError::Limit);
        }
        if need > self.data.len() {
            let room = crate::falloc::grow_capacity(self.data.len().max(FIRST_ROOM), need, 1)
                .or_oom()?
                .min(limit);
            let ghost before = self.view(); // [verus]
            // [canary]
            self.grow_to(room)?;
            assert(self.view() =~= before); // [verus]
        }
        let ghost before = self.data@; // [verus]
        self.data.as_mut_slice()[self.len..need].copy_from_slice(bytes);
        assert(self.data@.subrange(0, self.len as int) =~= before.subrange(0, self.len as int)); // [verus]
        // [canary]
        self.len = need;
        assert(self.view() =~= old(self).view() + bytes@); // [verus]
        Ok(())
    }

    pub fn raise_limit(&mut self, max: usize)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).view() == old(self).view(), // [verus]
            limit_for(final(self).max) >= limit_for(old(self).max), // [verus]
    {
        if max > self.content_limit() {
            // [canary]
            self.max = max;
        }
    }

    pub fn reserve(&mut self, n: usize) -> (r: Result<(), BufError>)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).max == old(self).max, // [verus]
            final(self).view() == old(self).view(), // [verus]
            final(self).data@.len() >= old(self).data@.len(), // [verus]
            r.is_ok() ==> final(self).data@.len() >= n || final(self).data@.len() == limit_for(old(self).max), // [verus]
    {
        let n = n.min(self.content_limit());
        if n <= self.data.len() {
            // [canary]
            return Ok(());
        }
        // [canary]
        self.grow_to(n)
    }

    pub fn steal(&mut self) -> (r: OwnedBuf)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).view() == Seq::<u8>::empty(), // [verus]
            final(self).data@.len() == 0, // [verus]
            r.0@ == old(self).view(), // [verus]
    {
        self.data.truncate(self.len);
        self.len = 0;
        // [canary]
        OwnedBuf(core::mem::take(&mut self.data))
    }

    fn content_limit(&self) -> (r: usize)
        ensures // [verus]
            r == limit_for(self.max), // [verus]
    {
        Buf::content_limit_for(self.max)
    }

    pub fn content_limit_for(max: usize) -> (r: usize)
        ensures // [verus]
            r == limit_for(max), // [verus]
    {
        let soft = if max != 0 { max } else { BUF_DEFAULT_LIMIT };
        soft.min(BUF_HARD_MAX)
    }

    fn grow_to(&mut self, room: usize) -> (r: Result<(), BufError>)
        requires // [verus]
            old(self).wf(), // [verus]
            old(self).data@.len() <= room <= limit_for(old(self).max), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).max == old(self).max, // [verus]
            final(self).len == old(self).len, // [verus]
            final(self).view() == old(self).view(), // [verus]
            r.is_ok() ==> final(self).data@.len() == room, // [verus]
            r.is_err() ==> final(self).data@ == old(self).data@ && r == Err::<(), BufError>(BufError::Oom), // [verus]
    {
        let ghost before = self.data@; // [verus]
        if self.data.falloc_reserve_exact(room - self.data.len()).is_err() {
            // [canary]
            return Err(BufError::Oom);
        }
        if self.data.falloc_resize(room, 0).is_err() {
            // [canary]
            return Err(BufError::Oom);
        }
        assert(self.data@.subrange(0, self.len as int) =~= before.subrange(0, self.len as int)); // [verus]
        // [canary]
        Ok(())
    }
}

pub fn copy_small<T: Copy>(dst: &mut [T], src: &[T])
    requires // [verus]
        old(dst)@.len() == src@.len(), // [verus]
    ensures // [verus]
        final(dst)@ == src@, // [verus]
{
    #[inline(always)]
    fn two<const K: usize, T: Copy>(dst: &mut [T], src: &[T])
        requires // [verus]
            old(dst)@.len() == src@.len(), // [verus]
            K <= src@.len() <= 2 * K, // [verus]
        ensures // [verus]
            final(dst)@ == src@, // [verus]
    {
        let n = src.len();
        dst[..K].copy_from_slice(&src[..K]);
        dst[n - K..n].copy_from_slice(&src[n - K..]);
        // [canary]
        assert(dst@ =~= src@); // [verus]
    }
    match src.len() {
        0 => {}
        1 => dst[0] = src[0],
        2..=3 => two::<2, T>(dst, src),
        4..=7 => two::<4, T>(dst, src),
        8..=16 => two::<8, T>(dst, src),
        _ => dst.copy_from_slice(src),
    }
    // [canary]
    proof { // [verus]
        if src@.len() <= 1 { // [verus]
            assert(dst@ =~= src@); // [verus]
        } // [verus]
    } // [verus]
}

} // verus!

fn main() {}
