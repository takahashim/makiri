//! Holding Ruby's GVL, as a value, and the process-global cell it is the key
//! to - what the reference engines (`lexbor::selectors`, `lexbor::css_parser`)
//! keep their one Lexbor parser in. The shipping CSS engine keeps its cache
//! per thread instead (`css_cache`), and needs neither.
//!
//! A [`GvlCell`] is a `Mutex`, so it is sound on its own: what it guards is
//! reached by one thread at a time whatever the caller holds. The [`Gvl`] token
//! its [`borrow`](GvlCell::borrow) asks for is the discipline on top - every
//! use runs on one thread at a time, so the lock is never contended, and a
//! borrow that finds it held is a re-entrant one on the same thread, answered
//! as [`Busy`] rather than waited for. (The cell used to be an `UnsafeCell`
//! that only the GVL made sound, under an `unsafe impl Sync`; with a `Mutex`
//! the token proves nothing the cell depends on.)
//!
//! Outside Ruby (the Rust tests, the fuzz harnesses) [`Gvl::exclusive`] makes
//! the token, with a process-wide mutex. It does not exist in the extension
//! (the `ruby` feature outside `cfg(test)`), which runs no reference engine.
//!
//! The token does not stop a thread borrowing a cell twice: a callback could
//! reach the same engine again. The second borrow finds the `Mutex` held and
//! is [`Busy`], never a second `&mut` and never a deadlock (`try_lock`).

#![forbid(unsafe_code)]

use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, TryLockError};

/// Proof that this thread holds the GVL (outside Ruby: the stand-in lock).
///
/// Neither `Send` nor `Sync`, so neither it nor a reference to it can leave the
/// thread that made it.
pub struct Gvl {
    _not_send: PhantomData<*mut ()>,
}

impl Gvl {
    /// The stand-in for the GVL where there is no Ruby: blocks until no other
    /// thread holds it. Also in the crate's own test build, which never runs
    /// inside Ruby whatever its features.
    #[cfg(any(test, not(feature = "ruby")))]
    pub fn exclusive() -> Exclusive {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        Exclusive {
            /* A panic while holding it leaves nothing half-done that the next
             * holder must know about: each engine resets itself on unwind. */
            _guard: LOCK.lock().unwrap_or_else(|e| e.into_inner()),
            gvl: Gvl {
                _not_send: PhantomData,
            },
        }
    }
}

/// [`Gvl::exclusive`]'s lock, held until this drops.
#[cfg(any(test, not(feature = "ruby")))]
pub struct Exclusive {
    _guard: std::sync::MutexGuard<'static, ()>,
    gvl: Gvl,
}

#[cfg(any(test, not(feature = "ruby")))]
impl Deref for Exclusive {
    type Target = Gvl;
    fn deref(&self) -> &Gvl {
        &self.gvl
    }
}

/// A process-global reached only with a [`Gvl`].
///
/// Only a `Send` value can be one, as with a `Mutex`:
///
/// ```compile_fail,E0277
/// use makiri::gvl::GvlCell;
/// static CELL: GvlCell<Option<std::rc::Rc<u8>>> = GvlCell::new(None);
/// ```
pub struct GvlCell<T> {
    value: Mutex<T>,
}

/// A second borrow of a [`GvlCell`] on the thread that already holds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl<T> GvlCell<T> {
    pub const fn new(value: T) -> Self {
        GvlCell {
            value: Mutex::new(value),
        }
    }

    /// The one mutable borrow of the value, for as long as the token's borrow.
    /// [`Busy`] while another borrow is live. A borrow a panic unwound through
    /// left the lock poisoned, which is taken over: each engine resets itself
    /// on unwind, so nothing is half-done for the next holder.
    pub fn borrow<'a>(&'a self, _gvl: &'a Gvl) -> Result<GvlRef<'a, T>, Busy> {
        let guard = match self.value.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(Busy),
        };
        Ok(GvlRef {
            guard,
            _gvl: PhantomData,
        })
    }
}

/// A live borrow of a [`GvlCell`]; the cell is free again when this drops,
/// unwinding included.
pub struct GvlRef<'a, T> {
    guard: MutexGuard<'a, T>,
    _gvl: PhantomData<&'a Gvl>,
}

impl<T> Deref for GvlRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for GvlRef<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::*;

    #[test]
    fn a_second_borrow_on_one_thread_is_busy_not_aliased() {
        static CELL: GvlCell<u32> = GvlCell::new(0);
        let gvl = Gvl::exclusive();
        let mut first = CELL.borrow(&gvl).expect("the first borrow");
        *first += 1;
        assert_eq!(CELL.borrow(&gvl).err(), Some(Busy));
        drop(first);
        assert_eq!(*CELL.borrow(&gvl).expect("free again"), 1);
    }

    #[test]
    fn a_panic_in_a_borrow_frees_the_cell() {
        static CELL: GvlCell<u32> = GvlCell::new(0);
        let caught = std::panic::catch_unwind(|| {
            let gvl = Gvl::exclusive();
            let mut v = CELL.borrow(&gvl).expect("the first borrow");
            *v = 7;
            panic!("inside the borrow");
        });
        assert!(caught.is_err());
        let gvl = Gvl::exclusive();
        assert_eq!(*CELL.borrow(&gvl).expect("free after the unwind"), 7);
    }

    #[test]
    fn threads_borrow_one_after_another() {
        static CELL: GvlCell<Vec<u32>> = GvlCell::new(Vec::new());
        let threads: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    let gvl = Gvl::exclusive();
                    CELL.borrow(&gvl).expect("serialised").push(i);
                })
            })
            .collect();
        for t in threads {
            t.join().expect("the thread");
        }
        let gvl = Gvl::exclusive();
        let mut seen = CELL.borrow(&gvl).expect("free").clone();
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3]);
    }
}
