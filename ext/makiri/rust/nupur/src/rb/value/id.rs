//! Interned names: an [`Id`] as `rb_intern` makes one, and the [`HiddenId`]
//! of an instance variable no Ruby code can reach.

use crate::rb::error::Error;
use crate::rb::ruby::Ruby;

/// An interned name - a method's, an ivar's, a constant's - as `rb_intern`
/// makes it (`raw::intern`). Only the layer turns one into the raw `ID` it
/// hands C, so a caller can keep and compare one but never forge one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct Id(nupur_sys::ID);

impl Id {
    /// The interned name `name` (`rb_intern`); a new name grows the symbol
    /// table, so it can raise `NoMemoryError`.
    pub fn intern(ruby: &Ruby, name: &core::ffi::CStr) -> Result<Id, Error> {
        crate::raw::intern_cstr(ruby, name)
    }

    #[inline]
    pub(crate) const fn from_raw(id: nupur_sys::ID) -> Id {
        Id(id)
    }

    #[inline]
    pub(crate) const fn as_raw(self) -> nupur_sys::ID {
        self.0
    }
}

/// The name of a hidden instance variable: an [`Id`] interned from a name
/// that does not start with `@`, so no Ruby code can read or write it.
///
/// What the ivar reads and writes take ([`crate::rb::object::ivar_get`],
/// [`crate::rb::object::ivar_set`]): Ruby's Ractor checks on an ivar - of a
/// class or module, or of a shareable object, from another Ractor - apply to
/// an `@` name alone (`rb_is_instance_id`, in every release from 3.3), so
/// with one of these the read raises nothing in any Ractor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct HiddenId(Id);

impl HiddenId {
    /// The hidden name `name` (`rb_intern`): `ArgumentError` for one that
    /// starts with `@`, and `NoMemoryError` as [`Id::intern`].
    pub fn intern(ruby: &Ruby, name: &core::ffi::CStr) -> Result<HiddenId, Error> {
        if name.to_bytes().first() == Some(&b'@') {
            return Err(Error::new(
                crate::rb::Builtin::ArgumentError,
                "a hidden ivar's name does not start with @",
            ));
        }
        Id::intern(ruby, name).map(HiddenId)
    }

    #[inline]
    pub(crate) const fn as_raw(self) -> nupur_sys::ID {
        self.0.as_raw()
    }
}
