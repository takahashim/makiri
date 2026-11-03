//! [`EmbeddedType`]: a [`TypedType`] whose envelope is kept in the object's
//! own slot, and read only by copy.

#![allow(unsafe_code)]

use core::ffi::CStr;

use super::data_type::{typed_data, DataType};
use super::{Birth, Envelope, Payload, Root, TypedType};
use crate::rb::{Error, Value};

/// A [`TypedType`] whose envelope is kept IN the object's slot
/// (`RUBY_TYPED_EMBEDDABLE`): no allocation beside the object's own, and
/// none to free. Only for a small payload (see [`EMBED_MAX`]).
///
/// Compaction moves embedded data with its object, so a `&T` held across
/// anything that can run the collector - an allocation - would point at
/// where the data was. This type is what keeps that from being written: it
/// has no `get`. A reader takes [`read`](Self::read)'s copy - the payload is
/// `Copy` - and only the GC callbacks see the data in place.
pub struct EmbeddedType<T: Payload + Copy, const N: usize = 0>(TypedType<T, N>);

impl<T: Payload + Copy, const N: usize> EmbeddedType<T, N> {
    /// An embedded type deriving from `parent` (`None` for a base type),
    /// shareable or not, its envelope held to what an object's slot takes.
    const fn with_flags(
        name: &'static CStr,
        parent: Option<&'static DataType>,
        shareable: bool,
    ) -> EmbeddedType<T, N> {
        const { assert!(core::mem::size_of::<Envelope<T, N>>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<Envelope<T, N>>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_flags(name, parent, true, shareable))
    }

    /// A base type.
    pub const fn base(name: &'static CStr) -> EmbeddedType<T, N> {
        Self::with_flags(name, None, false)
    }

    /// A base type whose objects are shared between Ractors, as
    /// [`TypedType::shareable`]'s are.
    pub const fn shareable(name: &'static CStr) -> EmbeddedType<T, N>
    where
        T: Sync,
    {
        Self::with_flags(name, None, true)
    }

    /// A type deriving from `parent`, which holds the same payload and
    /// slots, shareable when `parent`'s objects are.
    ///
    /// `parent` is `'static`, as [`TypedType::derived`]'s.
    pub const fn derived(
        name: &'static CStr,
        parent: &'static EmbeddedType<T, N>,
    ) -> EmbeddedType<T, N> {
        Self::with_flags(name, Some(&parent.0.raw), parent.0.shareable)
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&'static self, v: Value) -> bool {
        self.0.is(v)
    }

    /// A copy of `v`'s payload and slots, or Ruby's own `TypeError`. The
    /// data is read in place and copied out at once: no reference into the
    /// object is held past this call.
    #[inline]
    pub fn read(&'static self, v: Value) -> Result<(T, [Value; N]), Error> {
        let p = typed_data(v, &self.0.raw)? as *const Envelope<T, N>;
        // SAFETY: `typed_data` verified the type; the envelope is copied out
        // (`T: Copy`) with nothing run between, and the slots hold live values
        // the caller's `v` keeps marked.
        let (data, roots) = unsafe { ((*p).data, (*p).roots) };
        // SAFETY: as above.
        Ok((data, roots.map(|r| Value::from_sys(unsafe { r.value() }))))
    }

    /// [`TypedType::wrap`], as a bare `Value`: an embedded payload is never
    /// borrowed (only [`read`](Self::read)'s copy), so there is nothing for
    /// a [`Typed`](super::Typed) to hold.
    pub fn wrap<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        birth: Birth<T, R, N>,
    ) -> Result<Value, Error> {
        self.0.wrap_value(ruby, birth)
    }
}

/// The largest envelope an embedded type takes: with `RTypedData`'s header it fits
/// the 80-byte slot, well inside Ruby's largest (640), so
/// `rb_data_typed_object_zalloc` embeds it. Were it not embedded, the object
/// would still be right - its data allocated apart, which Ruby frees.
pub const EMBED_MAX: usize = 40;

/// The strictest alignment an embedded `T` may have: its data starts right
/// after `RTypedData`'s header, which a slot places at an 8-byte boundary
/// only - not the 16 `wrap_built` allows for `xcalloc`'s data.
pub const EMBED_ALIGN: usize = 8;
