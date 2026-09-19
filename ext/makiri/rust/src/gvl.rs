//! Holding Ruby's GVL, as a value.
//!
//! The CSS engines are process-global and unsynchronised. What makes that sound
//! is that every use of them runs under the GVL, which serialises Ruby threads -
//! a fact that used to live only in `# Safety` sections, where a safe entry
//! point could forget it and nothing would say so. [`Gvl`] is that fact as an
//! argument: [`GvlCell::borrow`] asks for one, and a thread can only have one
//! while it holds the lock.
//!
//! - In the extension, `bridge::gvl::held` makes one from a `magnus::Ruby`
//!   handle, and `bridge::gvl::without_gvl` requires a `Send` body - a `Gvl` is
//!   not `Send`, so none can be carried into the GVL-released region.
//! - Outside Ruby (the Rust tests, the fuzz harnesses) there is no GVL, and
//!   [`Gvl::exclusive`] stands in for it with a process-wide mutex. It does not
//!   exist in the extension (the `ruby` feature outside `cfg(test)`), where it
//!   could run alongside a Ruby thread that holds the real lock.
//!
//! The token proves the lock, not that a thread borrows a cell only once: a
//! callback that re-enters Ruby could reach the same engine again. So
//! [`GvlCell`] also keeps a busy flag, and a second borrow is an error rather
//! than a second `&mut`.

#![allow(unsafe_code)]

use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

/// Proof that this thread holds the GVL (outside Ruby: the stand-in lock).
///
/// Neither `Send` nor `Sync`, so neither it nor a reference to it can leave the
/// thread that made it.
pub struct Gvl {
    _not_send: PhantomData<*mut ()>,
}

impl Gvl {
    /// # Safety
    /// The calling thread holds Ruby's GVL, and does not release it while the
    /// token lives.
    #[cfg(feature = "ruby")]
    pub(crate) unsafe fn assume() -> Gvl {
        Gvl {
            _not_send: PhantomData,
        }
    }

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
pub struct GvlCell<T> {
    busy: Cell<bool>,
    value: UnsafeCell<T>,
}

// SAFETY: every access goes through `borrow`, which takes a `&Gvl`. A token
// exists only on a thread holding the GVL (or the stand-in lock), so accesses
// from different threads are serialised - and ordered - by that lock, and the
// busy flag rules out a second borrow on one thread. `T` holds Lexbor pointers
// that are only ever used under the same lock, hence no `T: Send` bound.
unsafe impl<T> Sync for GvlCell<T> {}

/// A second borrow of a [`GvlCell`] on the thread that already holds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl<T> GvlCell<T> {
    pub const fn new(value: T) -> Self {
        GvlCell {
            busy: Cell::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// The one mutable borrow of the value, for as long as the token's borrow.
    /// [`Busy`] while another borrow is live.
    pub fn borrow<'a>(&'a self, _gvl: &'a Gvl) -> Result<GvlRef<'a, T>, Busy> {
        if self.busy.replace(true) {
            return Err(Busy);
        }
        Ok(GvlRef {
            cell: self,
            _gvl: PhantomData,
        })
    }
}

/// A live borrow of a [`GvlCell`]; the cell is free again when this drops,
/// unwinding included.
pub struct GvlRef<'a, T> {
    cell: &'a GvlCell<T>,
    _gvl: PhantomData<&'a Gvl>,
}

impl<T> Deref for GvlRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the busy flag makes this the only borrow of the value, and
        // the token that `borrow` took serialises it against other threads.
        unsafe { &*self.cell.value.get() }
    }
}

impl<T> DerefMut for GvlRef<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as `deref`, and `&mut self` makes this the only live
        // reference derived from this borrow.
        unsafe { &mut *self.cell.value.get() }
    }
}

impl<T> Drop for GvlRef<'_, T> {
    fn drop(&mut self) {
        self.cell.busy.set(false);
    }
}

#[cfg(test)]
mod tests {
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
}
