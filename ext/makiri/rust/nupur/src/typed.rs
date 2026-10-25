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

    /// The bytes held outside Ruby's heap that this object has reported to
    /// the GC ([`report_external`]) and not yet given back: the free callback
    /// gives exactly these back after the `Drop`. Read in the collector, so
    /// it must be a plain read.
    fn reported_external(&self) -> usize {
        0
    }
}

/// Tell the GC about `delta` bytes held outside its heap (negative: given
/// back). Memory Ruby does not allocate is otherwise invisible to it and
/// never triggers a collection. A growth may run one here, so nothing
/// borrowed from a Ruby object may be held across the call. What an object
/// reports it says in its [`Payload::reported_external`], and the free
/// callback gives it back.
pub fn report_external(_: &crate::rb::Ruby, delta: isize) {
    // SAFETY: a plain counter adjustment, with the GVL held.
    unsafe { rb_sys::rb_gc_adjust_memory_usage(delta as rb_sys::ssize_t) };
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
        let n = isize::try_from(reported).unwrap_or(isize::MAX);
        // SAFETY: a plain counter adjustment, from the free hook, where it
        // only subtracts - as Ruby's own `xfree` does from here.
        unsafe { rb_sys::rb_gc_adjust_memory_usage(0isize.saturating_sub(n) as rb_sys::ssize_t) };
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

unsafe extern "C" fn memsize_cb<T: Payload, const N: usize>(ptr: *const c_void) -> rb_sys::size_t {
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
        crate::caught::quietly(0, || data.memsize().saturating_add(slots)) as rb_sys::size_t
    })
}

/// A `rb_data_type_t` for a payload `T` with `N` slots: its GC callbacks
/// drive [`Payload`] for `T`, and every way to make or read an object of this
/// type goes through it.
///
/// The type parameters are the point. The raw API took a `*const
/// rb_data_type_t` beside a separate `T`, so wrapping a node's data under a
/// Document's type - and reading it back as the wrong struct - compiled. Here
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
                Some(memsize_cb::<T, N>),
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
    /// data - so the reference cannot outlive the VALUE it came from.
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
    pub fn wrap(
        &'static self,
        _: &crate::rb::Ruby,
        klass: crate::rb::RClass,
        data: T,
        roots: [Value; N],
    ) -> Result<Value, Error> {
        // SAFETY: under the GVL (`&Ruby`), and `ty` is this payload's own type.
        let obj = unsafe {
            wrap_built::<T, N>(
                klass.as_raw(),
                self.raw.as_ptr(),
                data,
                roots.map(|v| v.as_raw()),
            )
        }?;
        // SAFETY: the object just wrapped, live on this frame.
        Ok(unsafe { value(obj) })
    }
}

/// An object of a [`TypedType<T, N>`], its type checked once
/// ([`TypedType::check`]): a hot path that reads an object's data and slots
/// several times - a NodeSet's nodes, kind and Document per element - pays
/// for the check once.
///
/// Sound with no further check because an object's type never changes after
/// it is made, its data was found allocated by the check (and is never
/// freed before the object), and a non-embedded type's data, which is all
/// this reads, is its own allocation, which compaction does not move. The `Value` is the
/// caller's, on its stack, which keeps the object alive while the handle is
/// used (`rake dylint`'s `ruby_value_on_heap` sees it as a Ruby value).
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
    /// when it is made - a Document before its content.
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
    pub fn wrap_hidden(
        &'static self,
        ruby: &crate::rb::Ruby,
        klass: crate::rb::RClass,
        data: T,
        roots: [Value; N],
        ivars: &[(crate::helpers::ID, Value)],
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
    klass: crate::rb::RClass,
    ty: &'static TypedType<T, N>,
}

impl<T: Payload + 'static, const N: usize> Hidden<T, N> {
    /// The payload, borrowed from the object.
    pub fn data(&self) -> &T {
        self.ty.get_known(&self.obj)
    }

    /// The object, shown again with its class (`rb_obj_reveal`).
    pub fn reveal(self) -> Value {
        // SAFETY: the object `wrap_hidden` hid, and `klass` the class it was
        // made with; revealing writes only its class field.
        unsafe { rb_sys::rb_obj_reveal(self.obj.as_raw(), self.klass.as_raw()) };
        self.obj
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
    pub fn wrap(
        &'static self,
        ruby: &crate::rb::Ruby,
        klass: crate::rb::RClass,
        data: T,
        roots: [Value; N],
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
        DataType(rb_sys::rb_data_type_t {
            wrap_struct_name: name,
            function: rb_sys::rb_data_type_struct__bindgen_ty_1 {
                dmark,
                dfree,
                dsize,
                /* No compaction callback: an object's only values are its
                 * slots, marked pinned. */
                dcompact: None,
                reserved: [core::ptr::null_mut(); 1],
            },
            parent,
            data: core::ptr::null_mut(),
            flags: if embedded {
                FREE_IMMEDIATELY | EMBEDDABLE
            } else {
                FREE_IMMEDIATELY
            },
        })
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
                return Err(crate::helpers::type_error(
                    "an object whose data was never allocated",
                ));
            }
            return Ok(p);
        }
        match super::raw::check_typeddata(v, ty) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(crate::helpers::type_error("wrong argument type")),
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
/// established - a receiver its `TryConvert` checked, or the Document a
/// checked node holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// the method's trampoline makes it `Makiri::InternalError`, where a raise
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
/// Document reports its arena with, only adds to that pressure, so a loop of
/// parses with no Ruby allocation of a Document's data ran no GC at all
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
        return Err(crate::helpers::internal_error(
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
