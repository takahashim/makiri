//! [`DocCell`]: a Document's content, reachable only through a borrow guard.
//!
//! The Ruby bridge keeps each Document's parsed content in one of these
//! (`bridge::dom`), so a read ([`Read`]) and an edit ([`Write`]) are counted
//! borrows: an edit asked for while any read is held is refused, and so is a
//! read during an edit - as values ([`Conflict`]), never a panic. Ruby-free on
//! purpose, so the two properties the bridge relies on can be tested here,
//! where `cargo test` runs: what a guard lends cannot outlive it, and a guard
//! cannot leave its thread.
//!
//! A reference read through a guard ends with the guard:
//!
//! ```compile_fail,E0597
//! use makiri_engine::doc_cell::DocCell;
//! let cell = DocCell::new(Box::new(5u32));
//! let escaped: &u32 = {
//!     let read = cell.read().unwrap();
//!     &*read
//! };
//! println!("{escaped}");
//! ```
//!
//! A guard stays on the thread that took it:
//!
//! ```compile_fail,E0277
//! use makiri_engine::doc_cell::DocCell;
//! fn send<T: Send>(_: T) {}
//! let cell = DocCell::new(Box::new(5u32));
//! send(cell.read().unwrap());
//! ```
//!
//! And the cell itself is not shared between threads (a Document's content is
//! reached under the GVL of the one Ractor it belongs to, from the thread
//! that holds it - a Document is never shareable, so no other Ractor reaches
//! it):
//!
//! ```compile_fail,E0277
//! use makiri_engine::doc_cell::DocCell;
//! fn share<T: Sync>(_: &T) {}
//! share(&DocCell::new(Box::new(5u32)));
//! ```

#![forbid(unsafe_code)]

use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

/// Owned content, borrowed only through [`Read`] and [`Write`].
pub struct DocCell<T>(RefCell<Box<T>>);

/// Why a guard could not be taken: the content is held the other way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// A read was asked for while an edit holds the content.
    ByEdit,
    /// An edit was asked for while a read holds it.
    ByRead,
}

/// The content, borrowed shared.
pub struct Read<'a, T>(Ref<'a, Box<T>>);

/// The content, borrowed exclusively.
pub struct Write<'a, T>(RefMut<'a, Box<T>>);

impl<T> DocCell<T> {
    /// A cell owning `content`. Boxed already, so making the cell moves no
    /// content and allocates nothing.
    pub fn new(content: Box<T>) -> Self {
        DocCell(RefCell::new(content))
    }

    /// A shared borrow; [`Conflict::ByEdit`] while a [`Write`] is held.
    #[inline]
    pub fn read(&self) -> Result<Read<'_, T>, Conflict> {
        self.0.try_borrow().map(Read).map_err(|_| Conflict::ByEdit)
    }

    /// An exclusive borrow; [`Conflict::ByRead`] while any borrow is held.
    #[inline]
    pub fn write(&self) -> Result<Write<'_, T>, Conflict> {
        self.0
            .try_borrow_mut()
            .map(Write)
            .map_err(|_| Conflict::ByRead)
    }

    /// Whether any guard is held - a read or an edit. For the raise sweep's
    /// check that no raise left a borrow behind.
    #[inline]
    pub fn is_borrowed(&self) -> bool {
        self.0.try_borrow_mut().is_err()
    }

    /// `f` over the content when it can be read now, `None` while an edit
    /// holds it - for a question that must not wait or fail, as a GC
    /// callback's size is.
    #[inline]
    pub fn peek<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.0.try_borrow().ok().map(|c| f(&c))
    }
}

impl<T> Deref for Read<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> Deref for Write<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Write<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Conflict, DocCell};

    fn boxed(v: u32) -> Box<u32> {
        falloc::try_box(v).expect("a box")
    }

    #[test]
    fn reads_share_and_an_edit_waits_for_them() {
        let cell = DocCell::new(boxed(1));
        let a = cell.read().expect("a read");
        let b = cell.read().expect("a second read");
        assert_eq!(*a + *b, 2);
        assert_eq!(cell.write().err(), Some(Conflict::ByRead));
        drop(a);
        assert_eq!(cell.write().err(), Some(Conflict::ByRead));
        drop(b);
        *cell.write().expect("the edit, once no read is held") = 7;
        assert_eq!(*cell.read().expect("a read after"), 7);
    }

    #[test]
    fn an_edit_refuses_reads_and_other_edits() {
        let cell = DocCell::new(boxed(1));
        let w = cell.write().expect("the edit");
        assert_eq!(cell.read().err(), Some(Conflict::ByEdit));
        assert_eq!(cell.write().err(), Some(Conflict::ByRead));
        assert_eq!(cell.peek(|v| *v), None);
        drop(w);
        assert_eq!(cell.peek(|v| *v), Some(1));
    }

    /// A panic unwinding through a guard releases it: the guards are `Drop`,
    /// which unwinding runs (the crate builds with `panic = "unwind"`).
    #[test]
    fn a_panic_releases_the_guard_it_unwinds_through() {
        let cell = DocCell::new(boxed(1));
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut w = cell.write().expect("the edit");
            *w = 2;
            panic!("mid-edit");
        }));
        assert!(unwound.is_err());
        assert_eq!(*cell.read().expect("released"), 2);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _r = cell.read().expect("a read");
            panic!("mid-read");
        }));
        assert!(unwound.is_err());
        assert!(cell.write().is_ok(), "the read was released");
    }
}
