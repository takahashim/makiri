//! [`Spare`]: a vector's allocation kept for reuse, with nothing in it.
//!
//! The CSS matcher keeps its stacks from one query to the next in a
//! process-global (`css_cache`'s, in a [`GvlCell`](crate::gvl::GvlCell)), so a
//! warm query allocates nothing. Those stacks are typed by the DOM's node
//! handles, which borrow their document - an XML one is not `Sync` - so a
//! plain `Vec` of them is not `Send`, and the global could not be either. What
//! the global keeps between queries is never a node, though: only the
//! capacity. `Spare` is that, as a type: the vector it holds is empty
//! whatever its methods are called with, so it can move between threads
//! whatever `T` is.

#![allow(unsafe_code)]

/// A `Vec<T>` that is always empty: only its allocation is kept.
pub struct Spare<T>(Vec<T>);

// SAFETY: `Send` moves ownership of the value to another thread. Every
// constructor and method leaves the vector empty (`new` makes one, `take`
// leaves `Vec::new()`, `put` clears what it stores), so a `Spare` owns no `T`:
// moving it moves an allocation from the global allocator, which any thread
// may free, and dropping it drops no `T`. That holds for every `T`, which is
// the point - the `T`s a query pushes live and die in that query.
unsafe impl<T> Send for Spare<T> {}

impl<T> Spare<T> {
    pub const fn new() -> Self {
        Spare(Vec::new())
    }

    /// The kept allocation, as an empty vector; nothing is kept until `put`.
    pub fn take(&mut self) -> Vec<T> {
        core::mem::take(&mut self.0)
    }

    /// Keep `v`'s allocation, emptied.
    pub fn put(&mut self, mut v: Vec<T>) {
        v.clear();
        self.0 = v;
    }

    /// The kept capacity, for tests.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl<T> Default for Spare<T> {
    fn default() -> Self {
        Spare::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Spare;
    use crate::falloc::Reserve;
    use std::rc::Rc;

    #[test]
    fn keeps_the_allocation_and_nothing_else() {
        let mut s: Spare<Rc<u8>> = Spare::new();
        let mut v = s.take();
        v.falloc_reserve(8).expect("reserve");
        let one = Rc::new(1u8);
        v.push(Rc::clone(&one));
        s.put(v);
        assert!(s.capacity() >= 8);
        // The item was dropped when the vector was kept.
        assert_eq!(Rc::strong_count(&one), 1);
        let v = s.take();
        assert!(v.is_empty() && v.capacity() >= 8);
        assert_eq!(s.capacity(), 0);
    }

    /// A `Spare` of a `!Send` item is `Send`, which a `Vec` of one is not.
    #[test]
    fn is_send_whatever_it_holds_the_room_for() {
        fn send<T: Send>() {}
        send::<Spare<Rc<u8>>>();
    }
}
