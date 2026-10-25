//! Verus proofs over `src/raw_vec/` and the growth arithmetic it uses,
//! unbounded where `rake kani` is bounded.
//!
//! What is proved is the shipped code: `falloc::grow_capacity` and
//! `RawVec`'s `as_slice`, `reserve`, `grow_to` and `push` are copied, every
//! line only Verus needs ends in `// [verus]`, and `rake verus:sync` checks
//! that the rest of each body is the source's, token for token.
//!
//! The axiom is `Block`'s (`src/raw_vec/block.rs`, the module's only
//! `unsafe`): `cap` elements seen as a slice, a successful `resize` keeping
//! the first `cap` and a failed one changing nothing, `set` writing one,
//! `prefix` reading the first `n`. Below it is Ruby's allocator, which no
//! proof here reaches; Kani and Miri run the real `Block` over the standard
//! one.

use vstd::prelude::*;

verus! {

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

} // mod falloc

pub use falloc::fits;

#[derive(PartialEq, Eq)]
pub enum GrowError {
    Overflow,
    NoMemory,
}

/// `src/raw_vec/block.rs`'s `Block`, by its contract alone.
#[verifier::external_body]
#[verifier::reject_recursive_types(T)]
pub struct Block<T> {
    _t: core::marker::PhantomData<T>,
}

impl<T: Copy> Block<T> {
    /// The elements.
    pub uninterp spec fn view(&self) -> Seq<T>;

    #[verifier::external_body]
    pub fn cap(&self) -> (c: usize)
        ensures
            c == self.view().len(),
    {
        unimplemented!()
    }

    #[verifier::external_body]
    pub fn prefix(&self, n: usize) -> (s: &[T])
        requires
            n <= self.view().len(),
        ensures
            s@ == self.view().subrange(0, n as int),
    {
        unimplemented!()
    }

    #[verifier::external_body]
    pub fn set(&mut self, i: usize, v: T)
        requires
            i < old(self).view().len(),
        ensures
            final(self).view() == old(self).view().update(i as int, v),
    {
        unimplemented!()
    }

    #[verifier::external_body]
    pub fn resize(&mut self, new: usize) -> (r: Result<(), GrowError>)
        requires
            new > old(self).view().len(),
        ensures
            r.is_ok() ==> final(self).view().len() == new && final(self).view().subrange(
                0,
                old(self).view().len() as int,
            ) == old(self).view(),
            r.is_err() ==> final(self).view() == old(self).view(),
    {
        unimplemented!()
    }
}

/// The fields are the source's; public here so the specs below can name them.
#[verifier::reject_recursive_types(T)]
pub struct RawVec<T> {
    pub block: Block<T>,
    pub len: usize,
}

impl<T: Copy> RawVec<T> {
    /// The invariant: the first `len` of the block's elements are the
    /// array, and the block's bytes fit an `isize`.
    pub open spec fn wf(&self) -> bool {
        &&& self.len <= self.block.view().len()
        &&& self.block.view().len() * size_of::<T>() <= isize::MAX
    }

    /// The contents.
    pub open spec fn view(&self) -> Seq<T> {
        self.block.view().subrange(0, self.len as int)
    }

    pub fn as_slice(&self) -> (s: &[T])
        requires // [verus]
            self.wf(), // [verus]
        ensures // [verus]
            s@ == self.view(), // [verus]
    {
        // [canary]
        self.block.prefix(self.len)
    }

    pub fn reserve(&mut self, n: usize) -> (r: Result<(), GrowError>)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).view() == old(self).view(), // [verus]
            r.is_ok() ==> final(self).block.view().len() >= n, // [verus]
    {
        if n > self.block.cap() {
            // [canary]
            self.grow_to(n)?;
        }
        Ok(())
    }

    fn grow_to(&mut self, cap: usize) -> (r: Result<(), GrowError>)
        requires // [verus]
            old(self).wf(), // [verus]
            cap > old(self).block.view().len(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).len == old(self).len, // [verus]
            final(self).view() == old(self).view(), // [verus]
            r.is_ok() ==> final(self).block.view().len() == cap, // [verus]
    {
        let fits = match cap.checked_mul(core::mem::size_of::<T>()) {
            Some(bytes) => bytes <= isize::MAX as usize,
            None => false,
        };
        if !fits {
            return Err(GrowError::Overflow);
        }
        let ghost before = self.view(); // [verus]
        let r = // [verus]
        self.block.resize(cap)
        ; // [verus]
        proof { // [verus]
            assert(self.view() =~= before); // [verus]
        } // [verus]
        // [canary]
        r // [verus]
    }

    pub fn push(&mut self, item: T) -> (r: Result<(), GrowError>)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            r.is_ok() ==> final(self).view() == old(self).view().push(item), // [verus]
            r.is_err() ==> final(self).view() == old(self).view(), // [verus]
    {
        if self.len == self.block.cap() {
            let want = match self.len.checked_add(1) {
                Some(w) => w,
                None => return Err(GrowError::Overflow),
            };
            let new_cap =
                match crate::falloc::grow_capacity(self.block.cap(), want, core::mem::size_of::<T>()) {
                    Some(c) => c,
                    None => return Err(GrowError::Overflow),
                };
            // [canary]
            self.grow_to(new_cap)?;
        }
        let ghost before = self.view(); // [verus]
        self.block.set(self.len, item);
        self.len += 1;
        assert(self.view() =~= before.push(item)); // [verus]
        // [canary]
        Ok(())
    }
}

} // verus!

fn main() {}
