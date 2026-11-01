//! [`External`]: the memory an object holds outside Ruby's heap, as it is
//! reported to the GC.

#![allow(unsafe_code)]

/// The bytes an object holds outside Ruby's heap, as reported to the GC -
/// the one way a report is made, so what was reported is always what the
/// free callback gives back ([`Payload::reported_external`](super::Payload::reported_external) answers
/// [`reported`](Self::reported)).
///
/// Memory Ruby does not allocate is otherwise invisible to it and never
/// triggers a collection. A cell, so a payload read through `&T` reports.
#[derive(Default)]
pub struct External(core::cell::Cell<usize>);

impl External {
    pub const fn new() -> External {
        External(core::cell::Cell::new(0))
    }

    /// What has been reported and not given back.
    #[inline]
    pub fn reported(&self) -> usize {
        self.0.get()
    }

    /// The object holds `bytes` outside the heap now: the change since the
    /// last report goes to the GC. A change Ruby could not take whole - past
    /// an `ssize_t` - is not reported, and not recorded, so the two never
    /// disagree. The default collector only raises its malloc count here and
    /// collects at the next allocation (`rb_gc_adjust_memory_usage` is
    /// MEMOP_TYPE_REALLOC), but another may collect in the call, so what is
    /// borrowed across it must be what a collection neither frees nor moves:
    /// this payload, whose object its caller keeps (a receiver, a slot's
    /// value, a [`Typed`](super::Typed) handle) and which compaction does not move - never
    /// a String's bytes. The new figure is recorded first, for a collection
    /// that reads it.
    pub fn report(&self, ruby: &crate::rb::Ruby, bytes: usize) {
        let then = self.0.get();
        let delta = if bytes >= then {
            nupur_sys::ssize_t::try_from(bytes - then)
        } else {
            nupur_sys::ssize_t::try_from(then - bytes).map(|d| -d)
        };
        let Ok(delta) = delta else { return };
        if delta == 0 {
            return;
        }
        self.0.set(bytes);
        // SAFETY: a plain counter adjustment, with the GVL held (`&Ruby`).
        unsafe { nupur_sys::read::rb_gc_adjust_memory_usage(ruby.sys(), delta) };
    }

    /// [`report`](Self::report) `more` bytes than were.
    #[inline]
    pub fn grow(&self, ruby: &crate::rb::Ruby, more: usize) {
        self.report(ruby, self.0.get().saturating_add(more));
    }
}
