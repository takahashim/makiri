//! The TypedData objects, and the GC callbacks Ruby drives.
//!
//! A wrapped value is a Rust struct behind a `rb_data_type_t`. Ruby calls the
//! three function pointers in that struct directly, so they have the C calling
//! convention and receive a raw data pointer - the raw Ruby ABI, and therefore
//! this layer's job (see `glue/mod.rs`). The struct declares what to do through
//! [`Hooks`], and the callbacks below are the one monomorphised bridge from
//! Ruby's GC to it, so no glue module writes an `extern "C"` GC function.
//!
//! The wrapper struct lives in memory from `ruby_xmalloc` ([`wrap_built`]),
//! and the free callback runs the struct's `Drop` in place before handing that
//! memory back to `ruby_xfree` - `xfree` alone would skip the destructor, and
//! with it everything the struct owns.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;

use magnus::rb_sys::AsRawValue;
use magnus::{Error, Value};
use rb_sys::{rb_data_type_t, VALUE};

/* ---- a VALUE inside a wrapped object ---- */

/// A Ruby value stored inside a wrapped object's data, which that object's
/// [`Hooks::trace`] hands to the collector.
///
/// The rule it carries: a VALUE may go into a native struct only once the
/// struct is wrapped. Before the wrap no mark sees the struct, so a GC in the
/// wrap's own allocation could free the value or compaction move it from
/// under the stored copy (the `DocData.errors` bug). A `Held` holding a value
/// can only be made with a [`Store`], and a `Store` exists only where the
/// owner is already a Ruby object: in [`TypedType::wrap`]'s store step and
/// in [`TypedType::with_store`]. A struct is built holding [`Held::empty`].
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
/// A cell, so compaction re-points it through a shared reference
/// ([`Tracer::movable`]): the compaction callback then never makes a `&mut`
/// of an object a method may be reading.
#[repr(transparent)]
pub(in crate::bridge) struct Held(core::cell::Cell<VALUE>);

impl Held {
    /// What a struct is built with: `Qfalse`, which a mark ignores.
    pub(in crate::bridge) const fn empty() -> Held {
        Held(core::cell::Cell::new(rb_sys::Qfalse as VALUE))
    }

    /// Hold `new`'s value in place of this one: a field that changes after
    /// the wrap (an `XPathContext`'s node). `new` comes from a [`Store`], so
    /// the rule [`Held`] carries holds for it too.
    #[inline]
    pub(in crate::bridge) fn replace(&self, new: Held) {
        self.0.set(new.0.get());
    }

    /// The value.
    ///
    /// # Safety
    /// `self` is a field of a live wrapped object whose [`Hooks::trace`]
    /// traces it, and the caller keeps that object reachable for the call (a
    /// method argument, a VALUE on the stack). Then the value is marked, so
    /// live - pinned when traced [`Tracer::pinned`], and current after any
    /// move when traced [`Tracer::movable`], which relocates it too.
    #[inline]
    pub(in crate::bridge) unsafe fn get(&self) -> Value {
        // SAFETY: the caller's contract - a marked value of a reachable object.
        unsafe { super::ruby::value(self.0.get()) }
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
        Held(core::cell::Cell::new(v.as_raw()))
    }
}

/// A GC phase, handed to [`Hooks::trace`]: the one walk over an object's
/// stored values that both the mark and the compaction callback make.
///
/// One walk, not two methods, so a value cannot be marked movable and then
/// left out of the relocation that must follow it: the field says how it is
/// held ([`Tracer::pinned`] or [`Tracer::movable`]) once, and each phase does
/// its half of that.
pub struct Tracer(Phase);

#[derive(Clone, Copy)]
enum Phase {
    Mark,
    Relocate,
}

impl Tracer {
    /// A value the object holds once, PINNED where it is: marked, and never
    /// moved, so never relocated.
    ///
    /// A table of many - a node cache - uses [`Tracer::movable`] instead, so
    /// 50,000 pinned objects do not stop compaction from doing its job.
    #[inline]
    pub(in crate::bridge) fn pinned(&self, v: &Held) {
        if let Phase::Mark = self.0 {
            // SAFETY: called from Ruby's mark phase, with the GVL held.
            unsafe { rb_sys::rb_gc_mark(v.0.get()) };
        }
    }

    /// A value compaction MAY move: marked movable, then re-pointed at where
    /// its object is now.
    #[inline]
    pub(in crate::bridge) fn movable(&self, v: &Held) {
        match self.0 {
            // SAFETY: called from Ruby's mark phase, with the GVL held.
            Phase::Mark => unsafe { rb_sys::rb_gc_mark_movable(v.0.get()) },
            // SAFETY: called from Ruby's compaction phase, with the GVL held,
            // on a value the mark phase marked movable - the same walk did.
            Phase::Relocate => v.0.set(unsafe { rb_sys::rb_gc_location(v.0.get()) }),
        }
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
    /// Hand every `VALUE` this object holds to `tracer`, each as it is held.
    ///
    /// Through `&self`: a [`Held`] relocates in place, so neither callback
    /// makes a `&mut` of an object a method may be reading when the collector
    /// runs. Ruby's `GC.auto_compact` (which `GC_COMPACT_STRESS=1` turns on)
    /// and `GC.verify_compaction_references` are what exercise the relocation.
    fn trace(&self, tracer: &Tracer);

    /// The bytes this object owns, reported to Ruby's GC.
    fn memsize(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

/* The three GC callbacks below are the crate's only `extern "C"` functions
 * with no panic latch, and deliberately so. They run from Ruby's collector,
 * where there is no frame to raise into: `mark_cb` cannot report a failure
 * without risking premature collection of what it did not mark, and `free_cb`
 * cannot report one without leaking or freeing twice. So a panic here keeps
 * the old behaviour - Rust turns the unwind at the boundary into an abort -
 * which is the honest answer when the alternative is a corrupted heap.
 * Keep their bodies trivial; that is what makes the choice cheap. */

unsafe extern "C" fn mark_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: Ruby hands back the pointer `wrap_built` stored, a live `T`.
    unsafe { (*(ptr as *const T)).trace(&Tracer(Phase::Mark)) };
}

unsafe extern "C" fn free_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: as `mark_cb`; the object is being collected, so this is its last
    // access - the `T` is dropped exactly once, here, and never read again -
    // and the struct came from `ruby_xmalloc`, so it goes back to `ruby_xfree`.
    unsafe {
        core::ptr::drop_in_place(ptr as *mut T);
        rb_sys::ruby_xfree(ptr);
    }
}

unsafe extern "C" fn free_embedded_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: as `free_cb`, for data embedded in the object's own slot: the
    // `T` is dropped, and the memory is the slot, which Ruby reclaims.
    unsafe { core::ptr::drop_in_place(ptr as *mut T) };
}

unsafe extern "C" fn memsize_cb<T: Hooks>(ptr: *const c_void) -> rb_sys::size_t {
    // SAFETY: as `mark_cb`.
    unsafe { (*(ptr as *const T)).memsize() as rb_sys::size_t }
}

unsafe extern "C" fn compact_cb<T: Hooks>(ptr: *mut c_void) {
    // SAFETY: as `mark_cb`; compaction has finished moving and is now fixing up
    // the references to what it moved.
    unsafe { (*(ptr as *const T)).trace(&Tracer(Phase::Relocate)) };
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
                Some(compact_cb::<T>),
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

    /// [`base`](Self::base) for a `T` kept IN the object's slot
    /// (`RUBY_TYPED_EMBEDDABLE`): no allocation beside the object's own, and
    /// none to free. Compaction then moves the data with the object, so a
    /// `&T` must not be held across anything that can run the collector - an
    /// allocation - and every reader copies out what it needs. Only for a
    /// small `T` (see [`EMBED_MAX`]).
    pub const fn embedded_base(name: *const c_char) -> TypedType<T> {
        const { assert!(core::mem::size_of::<T>() <= EMBED_MAX) };
        Self::with_parent(name, core::ptr::null(), true)
    }

    /// [`derived`](Self::derived) for an embedded `T`.
    pub const fn embedded_derived(name: *const c_char, parent: &TypedType<T>) -> TypedType<T> {
        const { assert!(core::mem::size_of::<T>() <= EMBED_MAX) };
        Self::with_parent(name, parent.raw.as_ptr(), true)
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

    /// The `T` behind an object whose type the caller established. The
    /// pointer is valid while `v` is rooted.
    fn known_ptr(&'static self, v: Value) -> *mut T {
        typed_data_known(v, &self.raw) as *mut T
    }

    /// Run `f` over the data of `v`, an object whose type the caller
    /// established, with a [`Store`]: `v` is wrapped, so a value stored into
    /// its data is marked from the moment it is stored. Shared: a type that
    /// stores into its data after the wrap keeps what it stores into in a
    /// cell.
    pub(in crate::bridge) fn with_store<R>(
        &'static self,
        v: Value,
        f: impl FnOnce(&T, &Store) -> R,
    ) -> R {
        f(self.get_known(&v), &Store(()))
    }

    /// Allocate Ruby memory for a `T`, `build` it there, wrap it as a `klass`
    /// object of this type, and only then let `store` write the VALUEs it
    /// holds - see [`wrap_built`] for why that order.
    ///
    /// # Safety
    /// Under the GVL; see [`wrap_built`] for what `build` may return.
    pub(in crate::bridge) unsafe fn wrap(
        &'static self,
        klass: VALUE,
        build: impl FnOnce() -> T,
        store: impl FnOnce(&mut T, &Store),
    ) -> Result<VALUE, Error> {
        wrap_built::<T>(klass, self.raw.as_ptr(), build, store)
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
        dcompact: rb_sys::RUBY_DATA_FUNC,
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
                dcompact,
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
/// established - a receiver magnus converted, or the Document a checked node
/// holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// magnus turns it into a fatal error, where a raise would longjmp past them.
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

/// Allocate `ruby_xmalloc` memory for a `T`, write the fully built value
/// `build` returns into it, wrap it as a `klass` object of data type `ty`, and
/// only then let `store` write the VALUEs it holds.
///
/// The order is the point. The wrap allocates, so it is a GC point, and a VALUE
/// already sitting in this malloc'd struct is seen by no mark there: a GC can
/// free it, or compaction move it out from under the stored copy. So the built
/// value has its VALUE fields `Qfalse` (0), which the mark ignores, until
/// `store` sets them; and the VALUEs `store` writes are still on the caller's
/// stack across the wrap, where the conservative scan pins them.
///
/// Built AFTER the allocation, by a closure, not passed in, so a failed
/// allocation builds nothing. Both allocations come back as `Err` (`raw`'s
/// calls are protected); a failed wrap drops the built `T` in place and frees
/// its memory. Once wrapped, the free callback drops it in place.
///
/// # Safety
/// Under the GVL. `build` returns a `T` that holds no VALUE but `Qfalse` (all
/// a `Held` can hold before a `Store`); `ty` must be `T`'s type (its free
/// callback drops a `T` and returns the memory to `ruby_xfree`).
unsafe fn wrap_built<T>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    build: impl FnOnce() -> T,
    store: impl FnOnce(&mut T, &Store),
) -> Result<VALUE, Error> {
    const EMBEDDABLE: VALUE = rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_EMBEDDABLE as VALUE;
    if (*ty).flags & EMBEDDABLE != 0 {
        return wrap_embedded(klass, ty, build, store);
    }
    /* `ruby_xmalloc` is malloc-aligned; nothing wrapped asks for more. */
    const { assert!(core::mem::align_of::<T>() <= 16) };
    let p = super::raw::xmalloc(core::mem::size_of::<T>())? as *mut T;
    p.write(build());
    let obj = match super::raw::typed_object_wrap(klass, p as *mut c_void, ty) {
        Ok(obj) => obj,
        Err(e) => {
            core::ptr::drop_in_place(p);
            super::alloc::free(p);
            return Err(e);
        }
    };
    /* Wrapped: from here the mark sees what `store` puts in. */
    store(&mut *p, &Store(()));
    Ok(obj)
}

/// The largest `T` an embedded type takes: with `RTypedData`'s header it fits
/// the 80-byte slot, well inside Ruby's largest (640), so `rb_data_typed_object_zalloc`
/// always embeds it - which `free_embedded_cb`, which frees no memory, relies on.
pub const EMBED_MAX: usize = 40;

/// [`wrap_built`] for an embedded type: the object is allocated with its data
/// zeroed in its own slot, `build`'s value is written there, and `store`
/// runs as before. Nothing runs between the allocation and the write - `build`
/// is plain Rust - so no collection sees the zeroed data.
unsafe fn wrap_embedded<T>(
    klass: VALUE,
    ty: *const rb_data_type_t,
    build: impl FnOnce() -> T,
    store: impl FnOnce(&mut T, &Store),
) -> Result<VALUE, Error> {
    let obj = super::raw::typed_object_zalloc(klass, core::mem::size_of::<T>(), ty)?;
    /* `EMBED_MAX` keeps every embedded `T` small enough to be embedded; a
     * Ruby that did not would have allocated the data apart, which the free
     * callback would leak - refused rather than handed out. */
    /* Embedded data starts right after `RTypedData`'s header - `RBasic`'s two
     * words, `type` and `typed_flag` - where the `data` pointer would be
     * (`RTYPEDDATA_GET_DATA`); data allocated apart is anywhere else. */
    let p = data_pointer(obj) as *mut T;
    if p as usize != obj as usize + 4 * core::mem::size_of::<usize>() {
        return Err(crate::bridge::ruby::internal_error(
            "an embeddable object was not embedded",
        ));
    }
    p.write(build());
    store(&mut *p, &Store(()));
    Ok(obj)
}

/* ---- magnus's TypedData, for NodeSet ----
 * NodeSet is a magnus `#[derive(TypedData)]` (its GC hooks are magnus's), not
 * a `TypedType`; this is its one unchecked accessor, kept beside the others.
 * magnus boxes the value and its free drops the `Box`, so NodeSet gets its
 * `Drop` the same way a `TypedType` object does. */

/// The wrapped Rust value behind a TypedData object, without magnus's
/// `rb_protect`.
///
/// `<&T>::try_convert` - and so every magnus method with a wrapped receiver -
/// runs `rb_check_typeddata` inside `rb_protect`, which is a `setjmp` per call.
/// That is the right default when a Rust caller wants a `Result`, but it is not
/// free: on the per-node path it measured about a quarter of the throughput of
/// the C it replaced (`Node#css` over 2000 nodes, `notes/node_set_ab.rb`).
///
/// # Safety
/// Only for a VALUE the caller built as a `T` itself (a mismatch panics). The
/// returned lifetime is unconstrained; the caller must keep `v` rooted.
pub(in crate::bridge) unsafe fn typed_data_unprotected<'a, T: magnus::TypedData>(
    v: VALUE,
) -> &'a T {
    /* magnus::DataType is #[repr(transparent)] over rb_data_type_t, so this
     * cast is what the repr promises; the accessor for it is crate-private. */
    let dt = T::data_type() as *const magnus::typed_data::DataType as *const rb_data_type_t;
    assert!(
        rb_sys::rb_typeddata_is_kind_of(v, dt) != 0,
        "a VALUE built as a NodeSet had a different type"
    );
    &*(data_pointer(v) as *const T)
}

/// magnus's `Ruby::wrap` for a `#[derive(TypedData)]` value (NodeSet), with
/// both allocations - the box and the object - answered as `Err` rather than
/// raised (magnus's raises). magnus's free callback drops the box, so the
/// object is made exactly as `wrap` makes it.
pub(in crate::bridge) fn wrap_magnus<T: magnus::TypedData>(data: T) -> Result<Value, Error> {
    let ruby = super::ruby::gvl_ruby();
    let class = T::class_for(&ruby, &data);
    let boxed = crate::falloc::try_box(data)
        .map_err(|_| Error::new(ruby.exception_no_mem_error(), "failed to allocate memory"))?;
    let dt = T::data_type() as *const magnus::typed_data::DataType as *const rb_data_type_t;
    let p = Box::into_raw(boxed);
    // SAFETY: `p` is the box magnus's free callback for `T` expects, and `dt`
    // is `T`'s data type (magnus::DataType is #[repr(transparent)] over it).
    match unsafe { super::raw::typed_object_wrap(class.as_raw(), p as *mut c_void, dt) } {
        // SAFETY: the object just wrapped.
        Ok(obj) => Ok(unsafe { super::ruby::value(obj) }),
        Err(e) => {
            // SAFETY: not wrapped, so the box is still ours alone.
            drop(unsafe { Box::from_raw(p) });
            Err(e)
        }
    }
}
