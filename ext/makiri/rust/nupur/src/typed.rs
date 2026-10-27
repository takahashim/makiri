//! The TypedData objects, and the GC callbacks Ruby drives.
//!
//! A wrapped value is a Rust struct behind a `rb_data_type_t`. Ruby calls the
//! function pointers in that struct directly, so they have the C calling
//! convention and receive a raw data pointer - the raw Ruby ABI, and therefore
//! this layer's job. A struct says what it is through [`Payload`], and the
//! callbacks below are the one monomorphised bridge from Ruby's GC to it, so
//! no other module writes an `extern "C"` GC function.
//!
//! What an object holds is an [`Envelope`]: the payload `T`, and `N` slots
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
//! `rb_data_typed_object_zalloc` ([`wrap_built`]) - in the object's own slot
//! for an embedded type, from `xcalloc` otherwise - and the free callback runs
//! the payload's `Drop` in place before the memory goes back: freeing alone
//! would skip the destructor, and with it everything the payload owns.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void, CStr};
use core::marker::PhantomData;

use crate::rb::value::AsRawValue;
use crate::rb::{Error, Value};
use rb_sys::{rb_data_type_t, VALUE};

/// A `Value` from a raw handle this module holds - a slot, or what a wrap
/// returned. With `raw.rs`, the only place a raw `VALUE` becomes a `Value`
/// (`rake dylint`'s `raw_ruby_outside_bridge`).
///
/// # Safety
/// `raw` is a live Ruby value of the current process, kept rooted by the
/// caller.
#[inline]
unsafe fn value(raw: VALUE) -> Value {
    // SAFETY: the caller's contract.
    unsafe { Value::from_raw(raw) }
}

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

/// The bytes an object holds outside Ruby's heap, as reported to the GC -
/// the one way a report is made, so what was reported is always what the
/// free callback gives back ([`Payload::reported_external`] answers
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
    /// MEMOP_TYPE_REALLOC), but another may collect in the call, so nothing
    /// borrowed from a Ruby object is held across it; the new figure is
    /// recorded first, for a collection that reads it.
    pub fn report(&self, _: &crate::rb::Ruby, bytes: usize) {
        let then = self.0.get();
        let delta = if bytes >= then {
            rb_sys::ssize_t::try_from(bytes - then)
        } else {
            rb_sys::ssize_t::try_from(then - bytes).map(|d| -d)
        };
        let Ok(delta) = delta else { return };
        if delta == 0 {
            return;
        }
        self.0.set(bytes);
        // SAFETY: a plain counter adjustment, with the GVL held (`&Ruby`).
        unsafe { rb_sys::rb_gc_adjust_memory_usage(delta) };
    }

    /// [`report`](Self::report) `more` bytes than were.
    #[inline]
    pub fn grow(&self, ruby: &crate::rb::Ruby, more: usize) {
        self.report(ruby, self.0.get().saturating_add(more));
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
    roots: [VALUE; N],
    data: T,
}

/* The GC callbacks below are the crate's only `extern "C"` functions with no
 * panic latch, and deliberately so. They run from Ruby's collector, where
 * there is no frame to raise into: `mark_cb` cannot report a failure without
 * risking premature collection of what it did not mark, and `free_cb` cannot
 * report one without leaking or freeing twice. So a panic here keeps the old
 * behaviour - Rust turns the unwind at the boundary into an abort - which is
 * the honest answer when the alternative is a corrupted heap. Keep their
 * bodies trivial; that is what makes the choice cheap.
 *
 * Each passes over a null data pointer. `rb_data_typed_object_zalloc` makes
 * the object before it allocates a non-embedded type's data, so a failed
 * allocation leaves an object with none; CRuby's collector does not call a
 * callback for it today, but that is its implementation, not its documented
 * contract, and one branch here means nothing rests on it. */

#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_gc_mark raises NoMemoryError only when the mark stack cannot grow, a longjmp this frame loses nothing to (the no_unwind guard's Drop does nothing unless unwinding) and every C extension's dmark is exposed to alike"
    )
)]
unsafe extern "C" fn mark_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMark, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: Ruby hands back the data pointer of an object `wrap_built`
        // made, which holds a live envelope (see `wrap_built`). Only the slots
        // are read: no reference to the payload is made, which a method may be
        // reading when the collector runs.
        let roots = unsafe { core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).roots).read() };
        for r in roots {
            // SAFETY: from Ruby's mark phase, with the GVL held; a slot holds
            // a live value (or `Qfalse`, which a mark ignores).
            unsafe { rb_sys::rb_gc_mark(r) };
        }
    })
}

/// Give back what the payload reported, once it is dropped.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: called only from the free callbacks, which Ruby runs with the GVL"
    )
)]
fn give_back(reported: usize) {
    if reported != 0 {
        let n = rb_sys::ssize_t::try_from(reported).unwrap_or(rb_sys::ssize_t::MAX);
        // SAFETY: a plain counter adjustment, from the free hook, where it
        // only subtracts - as Ruby's own `xfree` does from here.
        unsafe { rb_sys::rb_gc_adjust_memory_usage(n.saturating_neg()) };
    }
}

unsafe extern "C" fn free_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFree, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `mark_cb`; the object is being collected, so this is its
        // last access - the payload is dropped exactly once, here, and never
        // read again - and the data came from `rb_data_typed_object_zalloc`'s
        // `xcalloc`, so it goes back to `ruby_xfree`, as
        // `TypedData_Make_Struct`'s data does.
        /* The drop is a read and `quietly`: a type whose drop the compiler
         * cannot show unwind-free (a tree's recursive glue) is caught rather
         * than unwound into the collector. */
        unsafe {
            let data = core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).data).read();
            let reported = crate::caught::quietly(0, || data.reported_external());
            crate::caught::quietly((), || drop(data));
            give_back(reported);
            rb_sys::ruby_xfree(ptr);
        }
    })
}

unsafe extern "C" fn free_embedded_cb<T: Payload, const N: usize>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFreeEmbedded, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `free_cb`, for an embeddable type: the payload is dropped
        // and the memory is left to Ruby - the object's own slot when it was
        // embedded, and data Ruby frees after this callback when it was not.
        let data = unsafe { core::ptr::addr_of!((*(ptr as *const Envelope<T, N>)).data).read() };
        let reported = crate::caught::quietly(0, || data.reported_external());
        crate::caught::quietly((), || drop(data));
        give_back(reported);
    })
}

/// The bytes the object owns beyond what Ruby counts itself. For a plain
/// type that is the whole envelope (Ruby counts its slot, not the data it
/// points at); for an embeddable one Ruby counts the envelope - as part of
/// the slot it is embedded in, or by the allocation's own size when it is
/// not - so only what the payload owns beyond its struct is left.
unsafe extern "C" fn memsize_cb<T: Payload, const N: usize, const EMBEDDABLE: bool>(
    ptr: *const c_void,
) -> rb_sys::size_t {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMemsize, _>(|| {
        if ptr.is_null() {
            return 0;
        }
        /* `quietly`: the size is advisory, so a panic in it - an overflow
         * check in a sum, say - answers 0 rather than unwinding into the
         * collector. (Not so for mark: a value left unmarked is freed while
         * still referenced, so it must be unwind-free itself.) */
        // SAFETY: as `mark_cb`.
        let data = unsafe { &(*(ptr as *const Envelope<T, N>)).data };
        let slots = core::mem::size_of::<[VALUE; N]>();
        let bytes = crate::caught::quietly(0, || {
            if EMBEDDABLE {
                data.memsize().saturating_sub(core::mem::size_of::<T>())
            } else {
                data.memsize().saturating_add(slots)
            }
        });
        rb_sys::size_t::try_from(bytes).unwrap_or(rb_sys::size_t::MAX)
    })
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
        unsafe { rb_sys::rb_typeddata_is_kind_of(v.as_raw(), self.raw.as_ptr()) != 0 }
    }

    /// The payload behind `v`, or Ruby's own `TypeError`.
    ///
    /// Borrowed for as long as the caller borrows `v`, the object that owns the
    /// data - so the reference cannot outlive the VALUE it came from. That
    /// borrow is the type checker's alone: the object must be kept alive
    /// as well - by Ruby (a receiver, an argument, a slot's owner) or by
    /// [`crate::rb::value::gc_guard`] after the reference's last use.
    pub fn get<'v>(&'static self, v: &'v Value) -> Result<&'v T, Error> {
        let p = typed_data(*v, &self.raw)? as *const Envelope<T, N>;
        // SAFETY: `typed_data` verified the type, and the wrapper owns the data.
        Ok(unsafe { &(*p).data })
    }

    /// [`get`](Self::get) for a VALUE whose type the caller already
    /// established; a mismatch is a bug in that reasoning, and panics.
    pub fn get_known<'v>(&'static self, v: &'v Value) -> &'v T {
        // SAFETY: as `get`; `known_ptr` asserts the type.
        unsafe { &(*self.known_ptr(*v)).data }
    }

    /// Slot `I` of `v`, an object of this type whose type the caller already
    /// established (a mismatch panics). `I < N` is checked when it compiles.
    ///
    /// The value is live for as long as the caller can use it: the slot is
    /// marked while `v` is, and once the caller holds it, it is on the
    /// caller's stack too.
    #[inline]
    pub fn root_known<const I: usize>(&'static self, v: &Value) -> Value {
        const { assert!(I < N, "no such slot") };
        // SAFETY: as `get_known`; the slots were written by `wrap_built` and
        // hold live values.
        unsafe { value((*self.known_ptr(*v)).roots[I]) }
    }

    /// `v` checked once to be an object of this type (or one deriving from
    /// it), or Ruby's own `TypeError`: a [`Typed`], which reads the data and
    /// the slots with no check of its own.
    #[inline]
    pub fn check(&'static self, v: Value) -> Result<Typed<T, N>, Error> {
        typed_data(v, &self.raw)?;
        Ok(Typed {
            obj: v,
            _t: PhantomData,
        })
    }

    /// The envelope behind an object whose type the caller established. The
    /// pointer is valid while `v` is rooted.
    fn known_ptr(&'static self, v: Value) -> *const Envelope<T, N> {
        typed_data_known(v, &self.raw) as *const Envelope<T, N>
    }

    /// Wrap `data` as a `klass` object of this type, keeping `roots` alive in
    /// its slots. A failed allocation is `Err`, `data` dropped.
    ///
    /// The payload is built, and the roots held, before the object exists:
    /// they are on this frame through the allocation - where the GC's stack
    /// scan keeps the roots alive and in place - and both are written into
    /// the object right after it, with nothing run between ([`wrap_built`]).
    pub fn wrap<R: Root>(
        &'static self,
        _: &crate::rb::Ruby,
        klass: crate::rb::DataClass,
        data: T,
        roots: [R; N],
    ) -> Result<Value, Error> {
        // SAFETY: under the GVL (`&Ruby`), and `ty` is this payload's own type.
        let obj = unsafe {
            wrap_built::<T, N>(
                klass.as_raw(),
                self.raw.as_ptr(),
                data,
                roots.map(|v| sealed::Root::root_value(v).as_raw()),
            )
        }?;
        // SAFETY: the object just wrapped, live on this frame.
        Ok(unsafe { value(obj) })
    }
}

/// An object of a [`TypedType<T, N>`], its type checked once
/// ([`TypedType::check`]): a hot path that reads an object's data and slots
/// several times - a collection's elements and its owner, per element - pays
/// for the check once.
///
/// Sound with no further check because an object's type never changes after
/// it is made, its data was found allocated by the check (and is never
/// freed before the object), and a non-embedded type's data, which is all
/// this reads, is its own allocation, which compaction does not move. The `Value` is the
/// caller's, on its stack, which keeps the object alive while the handle is
/// used (`rake dylint`'s `ruby_value_on_heap` sees it as a Ruby value) - so
/// long as the handle is still used after the data is: a [`data`](Self::data)
/// borrow outliving the handle's last use needs the object kept by Ruby (a
/// receiver, an argument, a slot's owner) or by
/// [`crate::rb::value::gc_guard`], as [`TypedType::get`]'s does.
pub struct Typed<T: Payload + 'static, const N: usize> {
    obj: Value,
    _t: PhantomData<fn() -> T>,
}

impl<T: Payload + 'static, const N: usize> Clone for Typed<T, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Payload + 'static, const N: usize> Copy for Typed<T, N> {}

impl<T: Payload + 'static, const N: usize> Typed<T, N> {
    /// The object.
    #[inline]
    pub fn value(self) -> Value {
        self.obj
    }

    /// The payload, borrowed for as long as the handle is.
    #[inline]
    pub fn data(&self) -> &T {
        // SAFETY: `check` established the object's type, which is `T`'s with
        // `N` slots (a derived type holds the same); the data is the
        // envelope `wrap_built` wrote, and the object is alive while the
        // caller holds its `Value`.
        unsafe { &(*(data_pointer(self.obj.as_raw()) as *const Envelope<T, N>)).data }
    }

    /// Slot `I` (`I < N`, checked when it compiles).
    #[inline]
    pub fn root<const I: usize>(&self) -> Value {
        const { assert!(I < N, "no such slot") };
        // SAFETY: as `data`; the slots hold live values the object marks.
        unsafe { value((*(data_pointer(self.obj.as_raw()) as *const Envelope<T, N>)).roots[I]) }
    }
}

impl<T: Payload + 'static, const N: usize> TypedType<T, N> {
    /// [`wrap`](Self::wrap), and then the object HIDDEN (`rb_obj_hide`: no
    /// class, so `ObjectSpace` does not list it and no Ruby code can find it)
    /// until [`Hidden::reveal`]: for an object whose data is not complete
    /// when it is made - one whose content arrives after it.
    ///
    /// It is not hidden from birth. `ivars`, its hidden ivars (names without
    /// `@`), are written first, while it still has its class - generic ivar
    /// storage may read an object's class, so nothing is set on one that has
    /// none - and then it is hidden. What keeps Ruby code from finding it in
    /// between is that none runs there: the ivar writes may allocate, and so
    /// collect, but a collection runs no Ruby code. A step added between the
    /// wrap and the hide that can run Ruby code breaks that. A failed ivar
    /// write hides the object and returns the error; a failed allocation of
    /// a non-embedded type's data leaves an object of the class with no data,
    /// visible, which every reader refuses (`typed_data`); one never revealed
    /// stays hidden for the GC to free.
    pub fn wrap_hidden<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        klass: crate::rb::DataClass,
        data: T,
        roots: [R; N],
        ivars: &[(crate::rb::Id, Value)],
    ) -> Result<Hidden<T, N>, Error> {
        let obj = self.wrap(ruby, klass, data, roots)?;
        let written = ivars
            .iter()
            .try_for_each(|&(id, v)| crate::raw::ivar_set(obj, id, v));
        // SAFETY: the object just made, of ours; hiding writes only its class
        // field, which `reveal` restores.
        unsafe { rb_sys::rb_obj_hide(obj.as_raw()) };
        written?;
        Ok(Hidden {
            obj,
            klass,
            ty: self,
        })
    }
}

/// An object [`TypedType::wrap_hidden`] made, hidden until it is complete.
///
/// It hands out no `Value`: a hidden object has no class, so Ruby code given
/// one - a return value, an Array element - would crash on it. What a builder
/// does with it is reach its data ([`data`](Self::data)) and
/// [`reveal`](Self::reveal) it, which gives the object back as the class it
/// was made with.
pub struct Hidden<T: Payload + 'static, const N: usize> {
    obj: Value,
    klass: crate::rb::DataClass,
    ty: &'static TypedType<T, N>,
}

impl<T: Payload + 'static, const N: usize> Hidden<T, N> {
    /// The payload, borrowed from the object.
    pub fn data(&self) -> &T {
        self.ty.get_known(&self.obj)
    }

    /// The object, shown again with its class (`rb_obj_reveal`). Raises
    /// `NoMemoryError`, and the object then stays hidden, where no Ruby code
    /// finds it.
    pub fn reveal(self, _: &crate::rb::Ruby) -> Result<Value, Error> {
        crate::raw::obj_reveal(self.obj, crate::rb::ReprValue::as_value(self.klass))?;
        Ok(self.obj)
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
    /// A base type.
    pub const fn base(name: &'static CStr) -> EmbeddedType<T, N> {
        const { assert!(core::mem::size_of::<Envelope<T, N>>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<Envelope<T, N>>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_parent(
            name.as_ptr(),
            core::ptr::null(),
            true,
        ))
    }

    /// A type deriving from `parent`, which holds the same payload and slots.
    ///
    /// `parent` is `'static`, as [`TypedType::derived`]'s.
    pub const fn derived(
        name: &'static CStr,
        parent: &'static EmbeddedType<T, N>,
    ) -> EmbeddedType<T, N> {
        const { assert!(core::mem::size_of::<Envelope<T, N>>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<Envelope<T, N>>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_parent(
            name.as_ptr(),
            parent.0.raw.as_ptr(),
            true,
        ))
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
        Ok((data, roots.map(|r| unsafe { value(r) })))
    }

    /// [`TypedType::wrap`].
    pub fn wrap<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        klass: crate::rb::DataClass,
        data: T,
        roots: [R; N],
    ) -> Result<Value, Error> {
        self.0.wrap(ruby, klass, data, roots)
    }
}

/* ---- the raw machinery behind TypedType ---- */

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
struct DataType(rb_sys::rb_data_type_t);

// SAFETY: the contents are set once at compile time and never mutated. Ruby
// reads them from whichever thread holds the GVL.
unsafe impl Sync for DataType {}

/* The table of callbacks, in pointer-sized words: 5 up to Ruby 4.0 (dmark,
 * dfree, dsize, dcompact, reserved[1]) and 12 from 4.1 (+ handle_weak_references,
 * reserved[7]). Zero-initialising it is right for a layout whose extra words
 * are all optional callbacks or zero-filled reserve - which only someone who
 * has read the new header knows - so any other size stops the build here
 * instead of shipping a table nobody has checked. */
const FUNCTION_TABLE_WORDS: usize =
    size_of::<rb_sys::rb_data_type_struct__bindgen_ty_1>() / size_of::<usize>();
const _: () = assert!(
    FUNCTION_TABLE_WORDS == 5 || FUNCTION_TABLE_WORDS == 12,
    "rb_data_type_t's callback table has a layout nupur has not read: check Ruby's \
     include/ruby/internal/core/rtypeddata.h, then list its size here"
);

impl DataType {
    /// `parent` is null for a base type.
    pub const fn new(
        name: *const core::ffi::c_char,
        parent: *const rb_sys::rb_data_type_t,
        dmark: rb_sys::RUBY_DATA_FUNC,
        dfree: rb_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> rb_sys::size_t>,
        embedded: bool,
    ) -> DataType {
        const FREE_IMMEDIATELY: VALUE =
            rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE;
        const EMBEDDABLE: VALUE = rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_EMBEDDABLE as VALUE;
        /* Zero first, then set only the fields used: Ruby's partial-initialiser
         * convention (`reserved` "must be filled with zeros"), and what keeps
         * this compiling when Ruby adds a callback to the table, as 4.1 did
         * (`handle_weak_references`, `reserved[1]` -> `reserved[7]`). The
         * layout check above is what turns a layout nobody has read into a
         * build failure, not a silent one. */
        // SAFETY: `rb_data_type_t` is plain C data - pointers, options of
        // function pointers and an integer - for which all-zero is valid, and
        // is Ruby's own initial state for the fields not set below.
        let mut t: rb_sys::rb_data_type_t = unsafe { core::mem::zeroed() };
        t.wrap_struct_name = name;
        t.function.dmark = dmark;
        t.function.dfree = dfree;
        t.function.dsize = dsize;
        /* No compaction callback (`dcompact` stays None): an object's only
         * values are its slots, marked pinned. */
        t.parent = parent;
        t.data = core::ptr::null_mut();
        t.flags = if embedded {
            FREE_IMMEDIATELY | EMBEDDABLE
        } else {
            FREE_IMMEDIATELY
        };
        DataType(t)
    }

    /// The raw pointer the Ruby API wants.
    #[inline]
    pub const fn as_ptr(&self) -> *const rb_sys::rb_data_type_t {
        self as *const DataType as *const rb_sys::rb_data_type_t
    }
}

/// The data pointer of a TypedData object of type `ty` (or a type deriving
/// from it), or the `TypeError` Ruby's own check raises.
///
/// The type is tested first, so a well-typed object - every call on the normal
/// path - makes no protected call: its data pointer is read directly. Only a
/// mismatch runs `rb_check_typeddata` (protected), which is what keeps the
/// error message Ruby's own, word for word, on every supported Ruby.
///
/// The pointer is never null: an object whose data was never allocated is a
/// `TypeError` too. `rb_data_typed_object_zalloc` makes a non-embedded type's
/// object before its data, so an allocation that failed leaves an object of
/// the class with none, which `ObjectSpace` can still hand to Ruby code until
/// it is collected; every reader of a payload comes through here or
/// [`typed_data_known`].
fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: `v` is a live value and `ty` a registered data type; the data
    // pointer is read only once the type is known to match.
    unsafe {
        if rb_sys::rb_typeddata_is_kind_of(v, ty) != 0 {
            let p = data_pointer(v);
            if p.is_null() {
                return Err(crate::rb::error::type_error(
                    "an object whose data was never allocated",
                ));
            }
            return Ok(p);
        }
        match super::raw::check_typeddata(v, ty) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(crate::rb::error::type_error("wrong argument type")),
        }
    }
}

/// The data pointer of a TypedData object, read without a check.
///
/// # Safety
/// `v` is a live TypedData object.
#[inline]
unsafe fn data_pointer(v: VALUE) -> *mut c_void {
    use rb_sys::stable_api::StableApiDefinition;
    // SAFETY: the caller's contract.
    unsafe { rb_sys::stable_api::get_default().rtypeddata_get_data(v) }
}

/// The data pointer of a TypedData object whose type the caller has already
/// established - a receiver its `TryConvert` checked, or the owner a
/// checked node holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// the method's trampoline makes it the internal error (`rb::error::INTERNAL_ERROR`), where a raise
/// would longjmp past them.
fn typed_data_known(v: Value, ty: &'static DataType) -> *mut c_void {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: as in `typed_data`; the assert is the type check.
    unsafe {
        assert!(
            rb_sys::rb_typeddata_is_kind_of(v, ty) != 0,
            "a VALUE of an established type had a different one"
        );
        let p = data_pointer(v);
        /* An established object was checked (`typed_data`) or made whole
         * (`wrap_built`), so its data is there. */
        assert!(!p.is_null(), "an established object without its data");
        p
    }
}

/// Make a `klass` object of data type `ty` with room for an envelope, and
/// write `data` and `roots` into it: `TypedData_Make_Struct`, the C API's own
/// way to make an object with its data, for every type - embedded or not,
/// which only decides where Ruby puts the data (`rtypeddata_get_data` finds it
/// either way).
///
/// One C call, `rb_data_typed_object_zalloc`, makes the object and its data,
/// so one protected call per object. A non-embedded type's data is its
/// `xcalloc`, which also makes it a point where Ruby decides whether malloc
/// pressure calls for a collection: `rb_gc_adjust_memory_usage`, which a
/// payload reports external memory with, only adds to that pressure, so a loop of
/// wraps with no Ruby allocation of their data ran no GC at all
/// (`spec/gc_accounting_spec.rb`).
///
/// The order is the point. The data is zeroed until the envelope is written,
/// which is not a valid payload, so the payload is built BEFORE the object
/// exists, and nothing runs between the allocation and the write: no mark,
/// free or size callback can see the zeroed data, and a panic building the
/// payload leaves no object at all. The roots are on this frame through the
/// allocation (the caller's array), where the GC's stack scan keeps them
/// alive and unmoved, and are in the slots - where the mark finds them -
/// before anything else runs. A failed allocation - `Err`, from `raw`'s
/// protected call - drops the payload.
///
/// # Safety
/// Under the GVL; `ty` must be the type of `T` with `N` slots; each root is
/// a live value.
unsafe fn wrap_built<T, const N: usize>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    data: T,
    roots: [VALUE; N],
) -> Result<VALUE, Error> {
    const { assert!(core::mem::size_of::<Envelope<T, N>>() != 0) };
    const { assert!(core::mem::align_of::<Envelope<T, N>>() <= 16) }; /* malloc's alignment */
    let obj = super::raw::typed_object_zalloc(klass, core::mem::size_of::<Envelope<T, N>>(), ty)?;
    let p = data_pointer(obj) as *mut Envelope<T, N>;
    if p.is_null() {
        return Err(crate::rb::error::internal_error(
            "a TypedData object was made without its data",
        ));
    }
    p.write(Envelope { roots, data });
    /* The roots stayed on this frame until here; from here the mark sees
     * them in the slots. */
    core::hint::black_box(&roots);
    Ok(obj)
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

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn mark(_: *mut c_void) {}
    unsafe extern "C" fn free(_: *mut c_void) {}
    unsafe extern "C" fn size(_: *const c_void) -> rb_sys::size_t {
        0
    }

    /// The table's words past the three callbacks `DataType::new` sets are
    /// all zero - `dcompact`, the reserve (`reserved[1]` up to Ruby 4.0), and on
    /// 4.1 `handle_weak_references` and `reserved[7]` too - since a non-zero
    /// reserve is a callback Ruby would call.
    #[test]
    fn the_callback_table_leaves_what_it_does_not_set_zero() {
        let ty = DataType::new(
            c"nupur::test".as_ptr(),
            core::ptr::null(),
            Some(mark),
            Some(free),
            Some(size),
            false,
        );
        let table = &ty.0.function;
        assert!(table.dmark.is_some() && table.dfree.is_some() && table.dsize.is_some());
        assert!(table.dcompact.is_none());
        // SAFETY: the table is `FUNCTION_TABLE_WORDS` pointer-sized words of
        // plain data, read here as such.
        let words = unsafe {
            core::slice::from_raw_parts(table as *const _ as *const usize, FUNCTION_TABLE_WORDS)
        };
        assert!(words[3..].iter().all(|&w| w == 0), "{words:?}");
        assert_eq!(ty.0.flags, VALUE::from(FREE_IMMEDIATELY_FLAG));
    }

    const FREE_IMMEDIATELY_FLAG: u32 =
        rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as u32;
}
