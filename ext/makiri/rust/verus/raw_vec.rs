//! Verus proofs over the growth arithmetic and the logic of `RawVec`
//! (`src/raw_vec.rs`), unbounded where `rake kani` is bounded.
//!
//! `grow_capacity` is `src/falloc/mod.rs`'s, copied: every line Verus alone
//! needs ends in `// [verus]`, and `rake verus` checks that the rest is the
//! source's body token for token, so the copy cannot drift from what ships.
//!
//! `RawVec` below is a MODEL of `src/raw_vec.rs`, not a copy: the same steps
//! (`grow_to`, `reserve`, `push`) over a ghost memory, a `Seq<Option<T>>` of
//! the block's `cap` slots (`Some` once written). The one axiom is the
//! allocator's (`RawAlloc::realloc`): a success keeps the slots already there
//! and gives the rest uninitialised, a failure changes nothing. What is
//! proved is what the real code's two `unsafe` operations rest on - the write
//! at `len` is inside the block, the first `len` slots are written - and that
//! the contents are what was pushed.

use vstd::prelude::*;

verus! {

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
        match nc.checked_mul(2) {
            Some(next) if next.checked_mul(elem).is_some() => nc = next,
            _ => return Some(need),
        }
    }
    Some(nc)
}

#[derive(PartialEq, Eq)]
pub enum GrowError {
    Overflow,
    NoMemory,
}

/// The allocator's answer to a realloc: whether it succeeded. Which it does
/// is not known - the proofs hold for either.
#[verifier::external_body]
fn realloc_succeeds(old: usize, new: usize) -> bool {
    true
}

pub struct RawVec<T> {
    pub len: usize,
    pub cap: usize,
    /// `size_of::<T>()`.
    pub elem: usize,
    /// The block's slots, as the allocator's axiom has them.
    pub mem: Ghost<Seq<Option<T>>>,
}

impl<T> RawVec<T> {
    /// The invariant every method keeps: the block has `cap` slots, its
    /// byte size fits (`cap * elem <= isize::MAX`), and the first `len` are
    /// written.
    pub open spec fn wf(&self) -> bool {
        &&& self.len <= self.cap
        &&& self.mem@.len() == self.cap
        &&& self.cap * self.elem <= isize::MAX
        &&& forall|i: int| 0 <= i < self.len ==> (#[trigger] self.mem@[i]).is_some()
    }

    /// The contents: the first `len` slots.
    pub open spec fn view(&self) -> Seq<T> {
        Seq::new(self.len as nat, |i: int| self.mem@[i].unwrap())
    }

    pub fn new(elem: usize) -> (v: Self)
        ensures
            v.wf(),
            v.view() == Seq::<T>::empty(),
            v.elem == elem,
    {
        RawVec { len: 0, cap: 0, elem, mem: Ghost(Seq::empty()) }
    }

    /// `RawVec::grow_to`: reallocate to exactly `cap` (> `self.cap`) slots.
    fn grow_to(&mut self, cap: usize) -> (r: Result<(), GrowError>)
        requires
            old(self).wf(),
            cap > old(self).cap,
        ensures
            final(self).wf(),
            final(self).elem == old(self).elem,
            final(self).len == old(self).len,
            final(self).view() == old(self).view(),
            r.is_ok() ==> final(self).cap == cap,
            r.is_err() ==> final(self).cap == old(self).cap,
    {
        let fits = match cap.checked_mul(self.elem) {
            Some(b) => b <= isize::MAX as usize,
            None => false,
        };
        if !fits {
            return Err(GrowError::Overflow);
        }
        if !realloc_succeeds(self.cap, cap) {
            return Err(GrowError::NoMemory);
        }
        /* The allocator's axiom: the slots there are kept, the new ones
         * come uninitialised. */
        let ghost old_mem = self.mem@;
        proof {
            self.mem = Ghost(old_mem + Seq::new((cap - self.cap) as nat, |_i: int| None));
        }
        self.cap = cap;
        assert(self.view() =~= old(self).view());
        Ok(())
    }

    /// `RawVec::reserve`.
    pub fn reserve(&mut self, n: usize) -> (r: Result<(), GrowError>)
        requires
            old(self).wf(),
        ensures
            final(self).wf(),
            final(self).view() == old(self).view(),
            r.is_ok() ==> final(self).cap >= n,
    {
        if n > self.cap {
            self.grow_to(n)?;
        }
        Ok(())
    }

    /// `RawVec::push`.
    pub fn push(&mut self, item: T) -> (r: Result<(), GrowError>)
        requires
            old(self).wf(),
        ensures
            final(self).wf(),
            r.is_ok() ==> final(self).view() == old(self).view().push(item),
            r.is_err() ==> final(self).view() == old(self).view(),
    {
        if self.len == self.cap {
            let want = match self.len.checked_add(1) {
                Some(w) => w,
                None => return Err(GrowError::Overflow),
            };
            let new_cap = match grow_capacity(self.cap, want, self.elem) {
                Some(c) => c,
                None => return Err(GrowError::Overflow),
            };
            self.grow_to(new_cap)?;
        }
        /* The real code's `self.ptr.add(self.len).write(item)`: in bounds. */
        assert(self.len < self.cap);
        let ghost before = self.view();
        proof {
            self.mem = Ghost(self.mem@.update(self.len as int, Some(item)));
        }
        self.len = self.len + 1;
        assert(self.view() =~= before.push(item));
        Ok(())
    }

    /// What the real `as_slice`'s `from_raw_parts(ptr, len)` rests on: every
    /// slot it reads is written.
    pub proof fn as_slice_reads_written_slots(&self)
        requires
            self.wf(),
        ensures
            forall|i: int| 0 <= i < self.len ==> (#[trigger] self.mem@[i]).is_some(),
            self.view().len() == self.len,
    {
    }
}

} // verus!

fn main() {}
