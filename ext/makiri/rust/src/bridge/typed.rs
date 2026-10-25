//! The TypedData objects, and the GC callbacks Ruby drives.
//!
//! A wrapped value is a Rust struct behind a `rb_data_type_t`. Ruby calls the
//! three function pointers in that struct directly, so they have the C calling
//! convention and receive a raw data pointer - the raw Ruby ABI, and therefore
//! this layer's job (see `glue/mod.rs`). The struct declares what to do through
//! [`Hooks`], and the callbacks below are the one monomorphised bridge from
//! Ruby's GC to it, so no glue module writes an `extern "C"` GC function.
//!
//! The wrapper struct is the object's data, made with the object by
//! `rb_data_typed_object_zalloc` ([`wrap_built`]) - in the object's own slot
//! for an embedded type, from `xcalloc` otherwise - and the free callback runs
//! the struct's `Drop` in place before the memory goes back: freeing alone
//! would skip the destructor, and with it everything the struct owns.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;

use crate::bridge::rb::value::AsRawValue;
use crate::bridge::rb::{Error, Value};
use rb_sys::{rb_data_type_t, VALUE};

/// A `Value` from a raw handle this module holds - a stored field, or what a
/// wrap returned. With `raw.rs`, the only place a raw `VALUE` becomes a
/// `Value` (`rake dylint`'s `raw_ruby_outside_bridge`).
///
/// # Safety
/// `raw` is a live Ruby value of the current process, kept rooted by the
/// caller.
#[inline]
unsafe fn value(raw: VALUE) -> Value {
    // SAFETY: the caller's contract.
    unsafe { Value::from_raw(raw) }
}

/* ---- a VALUE inside a wrapped object ---- */

/// A Ruby value stored inside a wrapped object's data, which that object's
/// [`Hooks::trace`] hands to the collector.
///
/// The rule it carries: a VALUE may go into a native struct only once the
/// struct is wrapped. Before the wrap no mark sees the struct, so a GC in the
/// wrap's own allocation could free the value or compaction move it from
/// under the stored copy (the `DocData.errors` bug). A `Held` holding a value
/// can only be made with a [`Store`], and a `Store` exists only where the
/// owner is already a Ruby object: in [`TypedType::wrap`]'s store step. A
/// struct is built holding [`Held::empty`], and a `Held` never changes after
/// that step - nothing re-points it, so it is marked pinned and never moved.
///
/// What the type enforces is the ORDER: the value goes in after the wrap. It
/// cannot enforce what comes after - that the `Held` stays where its owner's
/// trace finds it. Safe code holding a `Store` could put one into a `Vec` no
/// trace sees, or move it out of its field, and the owner's `Hooks::trace`
/// could forget it; any of those leaves a VALUE nothing keeps alive. So reading one
/// back ([`get`](Self::get)) is `unsafe`, with that as its contract, and the
/// type is visible to the bridge alone. Neither `Copy` nor `Clone`, so it is
/// read through its owner rather than carried off.
///
/// Only three are left, each a single field set once: a node wrapper's
/// Document, a Document's node table, a NodeSet's Document. Every other value
/// a wrapper keeps alive is a hidden ivar of its object (an XPathContext's
/// Document and node, a Document's errors), which the GC marks and moves
/// itself - where reading it per call would cost more than it saves.
#[repr(transparent)]
pub(in crate::bridge) struct Held(VALUE);

impl Held {
    /// What a struct is built with: `Qfalse`, which a mark ignores.
    pub(in crate::bridge) const fn empty() -> Held {
        Held(rb_sys::Qfalse as VALUE)
    }

    /// The value.
    ///
    /// # Safety
    /// `self` is a field of a live wrapped object whose [`Hooks::trace`]
    /// traces it, and the caller keeps that object reachable for the call (a
    /// method argument, a VALUE on the stack). Then the value is marked and
    /// pinned, so live and where it was stored.
    #[inline]
    pub(in crate::bridge) unsafe fn get(&self) -> Value {
        // SAFETY: the caller's contract - a marked value of a reachable object.
        unsafe { value(self.0) }
    }
}

/// Proof that the object a value is stored into is already wrapped - so its
/// mark sees the value from the moment it is stored. See [`Held`].
pub(in crate::bridge) struct Store(());

impl Store {
    /// `v`, as a value the wrapped object holds. Store the result in that
    /// object's data, where its mark finds it - see [`Held`].
    #[inline]
    pub(in crate::bridge) fn hold(&self, v: Value) -> Held {
        Held(v.as_raw())
    }
}

/// The mark phase, handed to [`Hooks::trace`].
///
/// There is no compaction phase: every [`Held`] is marked pinned, so nothing
/// a Rust struct holds ever moves, and there is no relocation to get wrong. A
/// table of many - the node table - is a Ruby Hash, which marks its entries
/// movable and relocates them itself.
pub struct Tracer(());

impl Tracer {
    /// A value the object holds, PINNED where it is: marked, and never moved.
    #[inline]
    pub(in crate::bridge) fn pinned(&self, v: &Held) {
        // SAFETY: called from Ruby's mark phase, with the GVL held.
        unsafe { rb_sys::rb_gc_mark(v.0) };
    }
}

/// A Rust value owned by a Ruby object.
///
/// Exactly one method is required: the walk over the Ruby values the object
/// keeps alive. `memsize` sizes the object for the GC and defaults to its
/// Rust size. What the object owns is released by its ordinary `Drop`, which
/// the free callback runs - so a `Drop` here runs inside Ruby's collector,
/// and must neither panic nor touch a Ruby object (see the note on the
/// callbacks below).
pub trait Hooks: Sized {
    /// Hand every [`Held`] this object holds to `tracer`. Through `&self`:
    /// the mark callback makes no `&mut` of an object a method may be
    /// reading when the collector runs.
    fn trace(&self, tracer: &Tracer);

    /// The bytes this object owns, reported to Ruby's GC.
    fn memsize(&self) -> usize {
        core::mem::size_of::<Self>()
    }
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

unsafe extern "C" fn mark_cb<T: Hooks>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMark, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: Ruby hands back the data pointer of an object `wrap_built`
        // made, which holds a live `T` (see `wrap_built`).
        unsafe { (*(ptr as *const T)).trace(&Tracer(())) };
    })
}

unsafe extern "C" fn free_cb<T: Hooks>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFree, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `mark_cb`; the object is being collected, so this is its
        // last access - the `T` is dropped exactly once, here, and never read
        // again - and the data came from `rb_data_typed_object_zalloc`'s
        // `xcalloc`, so it goes back to `ruby_xfree`, as
        // `TypedData_Make_Struct`'s data does.
        /* The drop is a read and `quietly`: a type whose drop the compiler
         * cannot show unwind-free (a tree's recursive glue) is caught rather
         * than unwound into the collector. */
        unsafe {
            let data = core::ptr::read(ptr as *mut T);
            crate::caught::quietly((), || drop(data));
            rb_sys::ruby_xfree(ptr);
        }
    })
}

unsafe extern "C" fn free_embedded_cb<T: Hooks>(ptr: *mut c_void) {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcFreeEmbedded, _>(|| {
        if ptr.is_null() {
            return;
        }
        // SAFETY: as `free_cb`, for an embeddable type: the `T` is dropped and
        // the memory is left to Ruby - the object's own slot when it was
        // embedded, and data Ruby frees after this callback when it was not.
        let data = unsafe { core::ptr::read(ptr as *mut T) };
        crate::caught::quietly((), || drop(data));
    })
}

unsafe extern "C" fn memsize_cb<T: Hooks>(ptr: *const c_void) -> rb_sys::size_t {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::GcMemsize, _>(|| {
        if ptr.is_null() {
            return 0;
        }
        /* `quietly`: the size is advisory, so a panic in it - an overflow
         * check in a sum, say - answers 0 rather than unwinding into the
         * collector. (Not so for mark: a value left unmarked is freed while
         * still referenced, so it must be unwind-free itself.) */
        // SAFETY: as `mark_cb`.
        let data = unsafe { &*(ptr as *const T) };
        crate::caught::quietly(0, || data.memsize()) as rb_sys::size_t
    })
}

/// A `rb_data_type_t` for `T`: its GC callbacks drive [`Hooks`] for `T`, and
/// every way to make or read an object of this type goes through it.
///
/// The type parameter is the point. The raw API took a `*const rb_data_type_t`
/// beside a separate `T`, so wrapping a `NodeData` under a Document's type -
/// and reading it back as the wrong struct - compiled. Here the data type and
/// the struct are one value, so the pairing cannot be wrong.
///
/// `const`, so a module keeps it in a `static`, as the C did.
pub struct TypedType<T: Hooks> {
    raw: DataType,
    _t: PhantomData<fn() -> T>,
}

impl<T: Hooks> TypedType<T> {
    const fn with_parent(
        name: *const c_char,
        parent: *const rb_data_type_t,
        embedded: bool,
    ) -> TypedType<T> {
        TypedType {
            raw: DataType::new(
                name,
                parent,
                Some(mark_cb::<T>),
                Some(if embedded {
                    free_embedded_cb::<T>
                } else {
                    free_cb::<T>
                }),
                Some(memsize_cb::<T>),
                embedded,
            ),
            _t: PhantomData,
        }
    }

    /// A base type.
    pub const fn base(name: *const c_char) -> TypedType<T> {
        Self::with_parent(name, core::ptr::null(), false)
    }

    /// A type deriving from `parent`, which holds the same `T`: an object of
    /// this type is also one of `parent`'s.
    pub const fn derived(name: *const c_char, parent: &TypedType<T>) -> TypedType<T> {
        Self::with_parent(name, parent.raw.as_ptr(), false)
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&self, v: Value) -> bool {
        // SAFETY: `v` is a live VALUE; the type check does not raise.
        unsafe { rb_sys::rb_typeddata_is_kind_of(v.as_raw(), self.raw.as_ptr()) != 0 }
    }

    /// The `T` behind `v`, or Ruby's own `TypeError`.
    ///
    /// Borrowed for as long as the caller borrows `v`, the object that owns the
    /// data - so the reference cannot outlive the VALUE it came from.
    pub fn get<'v>(&'static self, v: &'v Value) -> Result<&'v T, Error> {
        let p = typed_data(*v, &self.raw)? as *const T;
        // SAFETY: `typed_data` verified the type, and the wrapper owns the data.
        Ok(unsafe { &*p })
    }

    /// [`get`](Self::get) for a VALUE whose type the caller already
    /// established; a mismatch is a bug in that reasoning, and panics.
    pub fn get_known<'v>(&'static self, v: &'v Value) -> &'v T {
        // SAFETY: as `get`; `known_ptr` asserts the type.
        unsafe { &*self.known_ptr(*v) }
    }

    /// [`get_known`](Self::get_known) with the lifetime left to the caller:
    /// for a reference kept beside the object it reads, as a NodeSet's fill
    /// handle is.
    ///
    /// # Safety
    /// `v` must be an object of this type (a mismatch panics), and the
    /// caller must keep it rooted - on its stack - for as long as it uses
    /// the reference.
    pub(in crate::bridge) unsafe fn get_unbound<'a>(&'static self, v: Value) -> &'a T {
        // SAFETY: the caller's contract; `known_ptr` asserts the type.
        unsafe { &*self.known_ptr(v) }
    }

    /// The `T` behind an object whose type the caller established. The
    /// pointer is valid while `v` is rooted.
    fn known_ptr(&'static self, v: Value) -> *mut T {
        typed_data_known(v, &self.raw) as *mut T
    }

    /// Allocate Ruby memory for a `T`, `build` it there, wrap it as a `klass`
    /// object of this type, and only then let `store` write the VALUEs it
    /// holds - see [`wrap_built`] for why that order. A failed allocation is
    /// `Err`, the built `T` dropped.
    ///
    /// Safe because what [`wrap_built`] asks of `build` is the types':
    /// `&Ruby` is the GVL, and a `T` keeps a Ruby value only in a [`Held`],
    /// which holds nothing until a [`Store`] exists - and `store` runs after
    /// the wrap. A raw `VALUE` in a field would get around that; none is
    /// written outside `raw.rs` and this file (`rake dylint`'s
    /// `raw_ruby_outside_bridge`).
    pub(in crate::bridge) fn wrap(
        &'static self,
        _: &crate::bridge::rb::Ruby,
        klass: crate::bridge::rb::RClass,
        build: impl FnOnce() -> T,
        store: impl FnOnce(&mut T, &Store),
    ) -> Result<Value, Error> {
        // SAFETY: see above; `ty` is `T`'s own type, by construction.
        let obj = unsafe { wrap_built::<T>(klass.as_raw(), self.raw.as_ptr(), build, store) }?;
        // SAFETY: the object just wrapped, live on this frame.
        Ok(unsafe { value(obj) })
    }
}

/// What a reader of an embedded `T` gets: a copy of what it needs, never a
/// reference into the object.
///
/// Implemented by the module that owns `T`, beside its `trace`. `snapshot`
/// runs with a `&T` into the object's slot, so it must allocate nothing - a
/// field copy, a [`Held`] read.
pub trait Snapshot {
    type Copy: Copy;
    fn snapshot(&self) -> Self::Copy;
}

/// A [`TypedType`] whose `T` is kept IN the object's slot
/// (`RUBY_TYPED_EMBEDDABLE`): no allocation beside the object's own, and
/// none to free. Only for a small `T` (see [`EMBED_MAX`]).
///
/// Compaction moves embedded data with its object, so a `&T` held across
/// anything that can run the collector - an allocation - would point at
/// where the data was. This type is what keeps that from being written: it
/// has no `get`. A reader takes [`read`](Self::read)'s copy (`T::Copy`), and
/// the only `&T` it lends is to `T`'s own [`Snapshot`] and to the GC
/// callbacks, neither of which allocates.
pub struct EmbeddedType<T: Hooks + Snapshot>(TypedType<T>);

impl<T: Hooks + Snapshot> EmbeddedType<T> {
    /// A base type.
    pub const fn base(name: *const c_char) -> EmbeddedType<T> {
        const { assert!(core::mem::size_of::<T>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<T>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_parent(name, core::ptr::null(), true))
    }

    /// A type deriving from `parent`, which holds the same `T`.
    pub const fn derived(name: *const c_char, parent: &EmbeddedType<T>) -> EmbeddedType<T> {
        const { assert!(core::mem::size_of::<T>() <= EMBED_MAX) };
        const { assert!(core::mem::align_of::<T>() <= EMBED_ALIGN) };
        EmbeddedType(TypedType::with_parent(name, parent.0.raw.as_ptr(), true))
    }

    /// Whether `v` is an object of this type or one deriving from it. Never
    /// raises.
    #[inline]
    pub fn is(&self, v: Value) -> bool {
        self.0.is(v)
    }

    /// A copy of what `v`'s data holds, or Ruby's own `TypeError`.
    #[inline]
    pub fn read(&'static self, v: Value) -> Result<T::Copy, Error> {
        self.0.get(&v).map(T::snapshot)
    }

    /// [`TypedType::wrap`]. `store` is lent the data in place, so it must
    /// allocate nothing either: it stores the values `T` holds, no more.
    pub(in crate::bridge) fn wrap(
        &'static self,
        ruby: &crate::bridge::rb::Ruby,
        klass: crate::bridge::rb::RClass,
        build: impl FnOnce() -> T,
        store: impl FnOnce(&mut T, &Store),
    ) -> Result<Value, Error> {
        self.0.wrap(ruby, klass, build, store)
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
                /* No compaction callback: a struct holds only pinned
                 * values ([`Tracer`]). */
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
fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let (v, ty) = (v.as_raw(), ty.as_ptr());
    // SAFETY: `v` is a live value and `ty` a registered data type; the data
    // pointer is read only once the type is known to match.
    unsafe {
        if rb_sys::rb_typeddata_is_kind_of(v, ty) != 0 {
            return Ok(data_pointer(v));
        }
        match super::raw::check_typeddata(v, ty) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(crate::bridge::ruby::type_error("wrong argument type")),
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
        data_pointer(v)
    }
}

/// Make a `klass` object of data type `ty` with room for a `T`, write the
/// fully built value `build` returns into it, and only then let `store` write
/// the VALUEs it holds: `TypedData_Make_Struct`, the C API's own way to make
/// an object with its data, for every type - embedded or not, which only
/// decides where Ruby puts the data (`rtypeddata_get_data` finds it either
/// way).
///
/// One C call, `rb_data_typed_object_zalloc`, makes the object and its data,
/// so one protected call per object. A non-embedded type's data is its
/// `xcalloc`, which also makes it a point where Ruby decides whether malloc
/// pressure calls for a collection: `rb_gc_adjust_memory_usage`, which a
/// Document reports its arena with, only adds to that pressure, so a loop of
/// parses with no Ruby allocation of a Document's data ran no GC at all
/// (`spec/gc_accounting_spec.rb`).
///
/// The order is the point. The data is zeroed until the built value is
/// written, which is not a valid `T`, so the value is built BEFORE the object
/// exists: nothing runs between the allocation and the write, and a panic in
/// `build` leaves no object whose free callback would drop zeroed memory as a
/// `T`. A failed allocation - `Err`, from `raw`'s protected call - drops the
/// built value. And the value has its VALUE fields `Qfalse` until `store`
/// sets them, once the object exists, so a VALUE is never held where no mark
/// reaches it.
///
/// # Safety
/// Under the GVL. `build` runs no Ruby and returns a `T` that holds no VALUE
/// but `Qfalse` (all a `Held` can hold before a `Store`); `ty` must be `T`'s
/// type.
unsafe fn wrap_built<T>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    build: impl FnOnce() -> T,
    store: impl FnOnce(&mut T, &Store),
) -> Result<VALUE, Error> {
    const { assert!(core::mem::size_of::<T>() != 0) };
    const { assert!(core::mem::align_of::<T>() <= 16) }; /* malloc's alignment */
    let built = build();
    let obj = super::raw::typed_object_zalloc(klass, core::mem::size_of::<T>(), ty)?;
    let p = data_pointer(obj) as *mut T;
    if p.is_null() {
        return Err(crate::bridge::ruby::internal_error(
            "a TypedData object was made without its data",
        ));
    }
    p.write(built);
    /* An object: from here the mark sees what `store` puts in. */
    store(&mut *p, &Store(()));
    Ok(obj)
}

/// The largest `T` an embedded type takes: with `RTypedData`'s header it fits
/// the 80-byte slot, well inside Ruby's largest (640), so
/// `rb_data_typed_object_zalloc` embeds it. Were it not embedded, the object
/// would still be right - its data allocated apart, which Ruby frees.
pub const EMBED_MAX: usize = 40;

/// The strictest alignment an embedded `T` may have: its data starts right
/// after `RTypedData`'s header, which a slot places at an 8-byte boundary
/// only - not the 16 `wrap_built` allows for `xcalloc`'s data.
pub const EMBED_ALIGN: usize = 8;
