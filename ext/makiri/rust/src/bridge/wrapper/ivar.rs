//! A hidden instance variable's name - one without `@`, which no Ruby code
//! can read or write - interned once, at `Init_makiri`, by the module that
//! keeps the ivar.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, HiddenId, Ruby};

/// The name of a hidden ivar, interned by [`intern`](Self::intern) at
/// `Init_makiri` and read by [`id`](Self::id) ever after.
pub(in crate::bridge) struct HiddenIvar {
    name: &'static core::ffi::CStr,
    id: std::sync::OnceLock<HiddenId>,
}

impl HiddenIvar {
    pub(in crate::bridge) const fn new(name: &'static core::ffi::CStr) -> HiddenIvar {
        HiddenIvar {
            name,
            id: std::sync::OnceLock::new(),
        }
    }

    /// Intern the name; a second call keeps the first.
    pub(in crate::bridge) fn intern(&self, ruby: &Ruby) -> Result<(), Error> {
        if self.id.get().is_none() {
            let _ = self.id.set(HiddenId::intern(ruby, self.name)?);
        }
        Ok(())
    }

    /// The interned name.
    #[allow(
        clippy::expect_used,
        reason = "`Init_makiri` interns every name before it defines a method that reads one; \
                  unset is a broken invariant, a panic the trampoline makes an InternalError"
    )]
    pub(in crate::bridge) fn id(&self) -> HiddenId {
        *self
            .id
            .get()
            .expect("a hidden ivar's name was read before Init_makiri interned it")
    }
}
