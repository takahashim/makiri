//! The TypedData objects, and the GC callbacks Ruby drives.
//!
//! A wrapped value is a Rust struct behind a `rb_data_type_t`. Ruby calls the
//! function pointers in that struct directly, so they have the C calling
//! convention and receive a raw data pointer - the raw Ruby ABI, and therefore
//! this layer's job. A struct says what it is through [`Payload`], and the
//! callbacks (`callbacks`) are the one monomorphised bridge from Ruby's GC to it, so
//! no other module writes an `extern "C"` GC function.
//!
//! What an object holds is an `Envelope`: the payload `T`, and `N` slots
//! for the Ruby values the object keeps alive. The slots are this module's,
//! written once - in the same step as the payload, right after the object's
//! allocation - and never again, and the mark callback marks every one of
//! them pinned. So a payload holds no Ruby value at all (`rake dylint`'s
//! `ruby_value_on_heap` refuses one in a `Payload` type), and there is no
//! trace for a payload to forget: the `DocData.errors` crash - a value stored
//! before the object that marks it existed - and its mirror, a value stored
//! where no mark reaches it, have no code to happen in.
//!
//! The envelope is the object's data, made with the object by
//! `rb_data_typed_object_zalloc` (`wrap_built`) - in the object's own slot
//! for an embedded type, from `xcalloc` otherwise - and the free callback runs
//! the payload's `Drop` in place before the memory goes back: freeing alone
//! would skip the destructor, and with it everything the payload owns.

#![allow(unsafe_code)]

mod callbacks;
mod data_type;
mod external;

pub use external::External;

use callbacks::{free_cb, free_embedded_cb, mark_cb, memsize_cb};
use data_type::{data_pointer, typed_data, typed_data_known, wrap_built, DataType};

use core::ffi::{c_char, CStr};
use core::marker::PhantomData;

use crate::rb::value::AsSys;
use crate::rb::{Error, Value};
use nupur_sys::rb_data_type_t;
use nupur_sys::value::Root as Slot;

/// What an object's slot can be given ([`TypedType::wrap`]'s `roots`): any
/// Ruby value, or a value only a slot may keep - a hidden
/// [`IntTable`](crate::table::IntTable), which hands out no `Value` of its
/// own, since a hidden object has no class and Ruby code given one would
/// crash on it. Sealed: the value is read by this module alone.
pub trait Root: Copy + sealed::Root {}

impl<T: Copy + sealed::Root> Root for T {}

pub(crate) mod sealed {
    use crate::rb::{ReprValue, Value};

    /// [`super::Root`]'s value, out of reach of the crate's users.
    pub trait Root {
        fn root_value(self) -> Value;
    }

    impl<T: ReprValue> Root for T {
        #[inline]
        fn root_value(self) -> Value {
            self.as_value()
        }
    }

    impl Root for crate::table::IntTable {
        #[inline]
        fn root_value(self) -> Value {
            self.as_value()
        }
    }
}

/// A Rust value owned by a Ruby object: what [`TypedType`] wraps.
///
/// It holds no Ruby value - those are the object's slots (`roots` in
/// [`TypedType::wrap`]). What it owns is released by its ordinary `Drop`,
/// which the free callback runs - so a `Drop` here runs inside Ruby's
/// collector, and must neither panic nor call Ruby: memory reported to the
/// GC is given back by this module ([`Payload::reported_external`]), not by
/// the `Drop`.
pub trait Payload: Sized {
    /// The bytes this object owns, reported to Ruby's GC as its size.
    fn memsize(&self) -> usize {
        core::mem::size_of::<Self>()
    }

    /// What this object has reported to the GC and not yet given back - its
    /// [`External`]'s [`reported`](External::reported): the free callback
    /// gives exactly these back after the `Drop`. Read in the collector, so
    /// it must be a plain read.
    fn reported_external(&self) -> usize {
        0
    }
}

/// The bytes an object of payload `T` with `N` slots keeps as its data - its
/// envelope, padding included: what a size ratchet over a wrapper should
/// measure, since a sum of the parts misses the padding.
pub const fn envelope_size<T, const N: usize>() -> usize {
    core::mem::size_of::<Envelope<T, N>>()
}

/// What an object's data is: its `N` slots, then its payload.
///
/// `repr(C)`, so the slots come first at a fixed offset whatever `T` is.
#[repr(C)]
struct Envelope<T, const N: usize> {
    /// The values the object keeps alive, marked pinned by [`mark_cb`].
    /// Written once, by [`wrap_built`], and never again.
    roots: [Slot; N],
    data: T,
}

/// What a new object is made of: its class, its payload, and the values its
/// slots keep alive - one value, as they are one object's, which
/// [`TypedType::wrap`] and its kin make and write together (`wrap_built`).
pub struct Birth<T, R, const N: usize> {
    /// The class Ruby sees it as.
    pub klass: crate::rb::DataClass,
    /// The payload.
    pub data: T,
    /// What its slots keep alive.
    pub roots: [R; N],
}

/// A `rb_data_type_t` for a payload `T` with `N` slots: its GC callbacks
/// drive [`Payload`] for `T`, and every way to make or read an object of this
/// type goes through it.
///
/// The type parameters are the point. The raw API took a `*const
/// rb_data_type_t` beside a separate `T`, so wrapping a node's data under a
/// type for another struct - and reading it back as the wrong one - compiled. Here
/// the data type, the struct and the slot count are one value, so the pairing
/// cannot be wrong.
///
/// `const`, so a module keeps it in a `static`, as the C did.
pub struct TypedType<T: Payload, const N: usize = 0> {
    raw: DataType,
    _t: PhantomData<fn() -> T>,
}

impl<T: Payload, const N: usize> TypedType<T, N> {
    const fn with_parent(
        name: *const c_char,
        parent: *const rb_data_type_t,
        embedded: bool,
    ) -> TypedType<T, N> {
        TypedType {
            raw: DataType::new(
                name,
                parent,
                Some(mark_cb::<T, N>),
                Some(if embedded {
                    free_embedded_cb::<T, N>
                } else {
                    free_cb::<T, N>
                }),
                Some(if embedded {
                    memsize_cb::<T, N, true>
                } else {
                    memsize_cb::<T, N, false>
                }),
                embedded,
            ),
            _t: PhantomData,
        }
    }

    /// A base type. `name` is what Ruby calls the type in its messages and
    /// `ObjectSpace`, read as a C string for the life of the process - so a
    /// `'static` one, NUL-terminated by type.
    pub const fn base(name: &'static CStr) -> TypedType<T, N> {
        Self::with_parent(name.as_ptr(), core::ptr::null(), false)
    }

    /// A type deriving from `parent`, which holds the same payload and
    /// slots: an object of this type is also one of `parent`'s.
    ///
    /// `parent` is `'static`: its `rb_data_type_t`'s address is kept, and
    /// Ruby follows it for the life of the process.
    pub const fn derived(name: &'static CStr, parent: &'static TypedType<T, N>) -> TypedType<T, N> {
        Self::with_parent(name.as_ptr(), parent.raw.as_ptr(), false)
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&self, v: Value) -> bool {
        // SAFETY: `v` is a live VALUE; the type check does not raise.
        unsafe { nupur_sys::non_raising::rb_typeddata_is_kind_of(v.as_sys(), self.raw.as_ptr()) }
    }

    /// `Ok` for an object of this type (or one deriving from it), else Ruby's
    /// own `TypeError` - a type check that borrows nothing, for a caller that
    /// only needs to refuse another kind of value.
    #[inline]
    pub fn ensure(&'static self, v: Value) -> Result<(), Error> {
        typed_data(v, &self.raw).map(|_| ())
    }

    /// A handle on `v`, an object of this type whose type the caller already
    /// established - a node's Document from its slot, a receiver its method's
    /// conversion checked; a mismatch is a bug in that reasoning, and panics.
    ///
    /// The fast way in for those: one `rb_typeddata_is_kind_of`, asserted,
    /// where [`check`](Self::check) answers a `TypeError`. Its data is
    /// borrowed from the handle, as any [`Typed`]'s, so the object is held
    /// for as long as the borrow - there is no way from a bare `Value` to a
    /// `&T`, whose lifetime the GC would not see.
    #[inline]
    pub fn known(&'static self, v: Value) -> Typed<T, N> {
        self.known_ptr(v);
        Typed::new(v)
    }

    /// `v` checked once to be an object of this type (or one deriving from
    /// it), or Ruby's own `TypeError`: a [`Typed`], which holds the object and
    /// reads the data and the slots with no check of its own.
    #[inline]
    pub fn check(&'static self, v: Value) -> Result<Typed<T, N>, Error> {
        typed_data(v, &self.raw)?;
        Ok(Typed::new(v))
    }

    /// The envelope behind an object whose type the caller established. The
    /// pointer is valid while `v` is rooted.
    fn known_ptr(&'static self, v: Value) -> *const Envelope<T, N> {
        typed_data_known(v, &self.raw) as *const Envelope<T, N>
    }

    /// Make the object `birth` describes, of this type: its payload wrapped
    /// as an instance of its class, its roots kept alive in its slots. A
    /// failed allocation is `Err`, the payload dropped.
    ///
    /// The payload is built, and the roots held, before the object exists:
    /// they are on this frame through the allocation - where the GC's stack
    /// scan keeps the roots alive and in place - and both are written into
    /// the object right after it, with nothing run between (`wrap_built`).
    ///
    /// The object comes back as a [`Typed`]: nothing but this frame refers
    /// to it yet, and the handle holds it while its data is borrowed.
    pub fn wrap<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        birth: Birth<T, R, N>,
    ) -> Result<Typed<T, N>, Error> {
        self.wrap_value(ruby, birth).map(Typed::new)
    }

    /// [`wrap`](Self::wrap)'s object as a bare `Value`, for an embedded
    /// type, whose data is never borrowed.
    fn wrap_value<R: Root>(
        &'static self,
        _: &crate::rb::Ruby,
        birth: Birth<T, R, N>,
    ) -> Result<Value, Error> {
        let Birth { klass, data, roots } = birth;
        // SAFETY: under the GVL (`&Ruby`), and `ty` is this payload's own type.
        let obj = unsafe {
            wrap_built::<T, N>(
                klass.as_sys(),
                self.raw.as_ptr(),
                data,
                roots.map(|v| Slot::of(sealed::Root::root_value(v).as_sys())),
            )
        }?;
        Ok(Value::from_sys(obj))
    }
}

/// An object of a [`TypedType<T, N>`], its type established - checked by
/// [`TypedType::check`], or made by [`TypedType::wrap`] - and the handle its
/// data is borrowed through. Whatever the object is - a receiver, a value
/// read from somewhere, one just made that nothing else refers to - the
/// handle holds it where the GC sees it until the handle is dropped, which
/// is after the last borrow of [`data`](Self::data): borrow from the handle,
/// not from a copy of its [`value`](Self::value), and do not `mem::forget`
/// it. It is the only way to a `&T`: [`TypedType::check`] makes one for a
/// value Ruby gave, [`TypedType::known`] for one whose type is established,
/// and [`TypedType::wrap`] returns one for an object just made.
///
/// A hot path that reads an object's data and slots several times - a
/// collection's elements and its owner, per element - pays for the check
/// once. Sound with no further check because an object's type never changes
/// after it is made, its data was found allocated (and is never freed
/// before the object), and a non-embedded type's data, which is all this
/// reads, is its own allocation, which compaction does not move.
pub struct Typed<T: Payload + 'static, const N: usize> {
    obj: crate::rb::kept::Kept,
    _t: PhantomData<fn() -> T>,
}

impl<T: Payload + 'static, const N: usize> Typed<T, N> {
    /// A handle on `v`, whose type the caller established.
    #[inline]
    fn new(v: Value) -> Typed<T, N> {
        Typed {
            obj: crate::rb::kept::Kept::new(v),
            _t: PhantomData,
        }
    }

    /// The object, for handing to Ruby or storing where Ruby keeps it.
    #[inline]
    pub fn value(&self) -> Value {
        *self.obj.value()
    }

    /// The object's envelope, borrowed for as long as the handle is.
    #[inline]
    fn envelope(&self) -> &Envelope<T, N> {
        // SAFETY: the type was established when the handle was made, which
        // is `T`'s with `N` slots (a derived type holds the same); the data
        // is the envelope `wrap_built` wrote, and the handle holds the object
        // until after the borrow.
        unsafe { &*(data_pointer(self.value().as_sys()) as *const Envelope<T, N>) }
    }

    /// The payload, borrowed for as long as the handle is.
    #[inline]
    pub fn data(&self) -> &T {
        &self.envelope().data
    }

    /// Slot `I` (`I < N`, checked when it compiles).
    #[inline]
    pub fn root<const I: usize>(&self) -> Value {
        const { assert!(I < N, "no such slot") };
        // SAFETY: the slots hold live values the object marks.
        Value::from_sys(unsafe { self.envelope().roots[I].value() })
    }
}

impl<T: Payload + 'static, const N: usize> TypedType<T, N> {
    /// [`wrap`](Self::wrap) an object whose data is not complete when it is
    /// made - one whose content arrives after it - and keep it HIDDEN
    /// (`rb_obj_hide`: no class, so `ObjectSpace` does not list it and no
    /// Ruby code can find it) while `fill` completes the data; then show it
    /// with its class and return it.
    ///
    /// The object is never handed out before that: a hidden object has no
    /// class, and Ruby code given one - a return value, an Array element -
    /// would crash on it. `fill` sees only the data, and may run anything
    /// (a GVL-released parse included), since nothing can reach the object.
    /// An `Err` from `fill`, or a failure to show the object, leaves it
    /// hidden for the GC to free, and is returned. The object comes back as
    /// the [`Typed`] handle that held it through `fill`, so a caller that
    /// goes on using it before handing it to Ruby still holds it.
    ///
    /// It is not hidden from birth. `ivars`, its hidden ivars (names without
    /// `@`), are written first, while it still has its class - generic ivar
    /// storage may read an object's class, so nothing is set on one that has
    /// none - and then it is hidden. What keeps Ruby code from finding it in
    /// between is that none runs there: the ivar writes may allocate, and so
    /// collect, but a collection runs no Ruby code. A step added between the
    /// wrap and the hide that can run Ruby code breaks that. A failed
    /// allocation of a non-embedded type's data leaves an object of the class
    /// with no data, visible, which every reader refuses (`typed_data`).
    ///
    /// Nothing but this call refers to the object until it is shown, so its
    /// data is lent to `fill` through the handle the wrap made, held as a
    /// `Hidden` - which lends the data and shows the object, and hands out
    /// nothing else.
    pub fn wrap_hidden<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        birth: Birth<T, R, N>,
        ivars: &[(crate::rb::HiddenId, Value)],
        fill: impl FnOnce(&T) -> Result<(), Error>,
    ) -> Result<Typed<T, N>, Error> {
        let klass = birth.klass;
        let made = self.wrap(ruby, birth)?;
        let written = ivars
            .iter()
            .try_for_each(|&(id, v)| crate::raw::ivar_set(made.value(), id, v));
        // SAFETY: the object just made, which nothing but `made` refers to.
        let hidden = unsafe { Hidden::hide(made, klass) };
        written?;
        fill(hidden.data())?;
        hidden.show()
    }
}

/// An object [`TypedType::wrap_hidden`] has hidden: its data can be lent
/// and it can be shown, but its value cannot be had - a hidden object has no
/// class, and Ruby code given one would crash on it. Dropped unshown, it
/// stays hidden for the GC to free.
struct Hidden<T: Payload + 'static, const N: usize> {
    made: Typed<T, N>,
    klass: crate::rb::DataClass,
}

impl<T: Payload + 'static, const N: usize> Hidden<T, N> {
    /// Hide `made`, to be shown as an instance of `klass`.
    ///
    /// # Safety
    /// `made` is an object just made that nothing else refers to - no Ruby
    /// code holds it, and none will be handed it while it is hidden.
    unsafe fn hide(made: Typed<T, N>, klass: crate::rb::DataClass) -> Hidden<T, N> {
        // SAFETY: the caller's contract; hiding writes only the object's
        // class field, which `show` restores.
        unsafe { crate::rb::object::obj_hide(made.value()) };
        Hidden { made, klass }
    }

    /// The payload, lent while the object is hidden.
    fn data(&self) -> &T {
        self.made.data()
    }

    /// Show the object with its class again, and hand it out. A failure (out
    /// of memory) leaves it hidden, for the GC.
    fn show(self) -> Result<Typed<T, N>, Error> {
        crate::raw::obj_reveal(
            self.made.value(),
            crate::rb::ReprValue::as_value(self.klass),
        )?;
        Ok(self.made)
    }
}

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
    /// An embedded type deriving from `parent` (null for a base type), its
    /// envelope held to what an object's slot takes.
    const fn with_parent(name: &'static CStr, parent: *const rb_data_type_t) -> EmbeddedType<T, N> {
        const { assert!(core::mem::size_of::<Envelope<T, N>>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<Envelope<T, N>>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_parent(name.as_ptr(), parent, true))
    }

    /// A base type.
    pub const fn base(name: &'static CStr) -> EmbeddedType<T, N> {
        Self::with_parent(name, core::ptr::null())
    }

    /// A type deriving from `parent`, which holds the same payload and slots.
    ///
    /// `parent` is `'static`, as [`TypedType::derived`]'s.
    pub const fn derived(
        name: &'static CStr,
        parent: &'static EmbeddedType<T, N>,
    ) -> EmbeddedType<T, N> {
        Self::with_parent(name, parent.0.raw.as_ptr())
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&self, v: Value) -> bool {
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
    /// a [`Typed`] to hold.
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
