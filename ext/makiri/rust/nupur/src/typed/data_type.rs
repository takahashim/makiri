//! The raw machinery behind [`TypedType`](super::TypedType): the
//! `rb_data_type_t` it carries, reading an object's data pointer, and making
//! an object with its data.

#![allow(unsafe_code)]

use core::ffi::c_void;

use nupur_sys::{rb_data_type_t, VALUE};

use super::Envelope;
use crate::rb::value::AsSys;
use crate::rb::{Error, Value};

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
pub(super) struct DataType(nupur_sys::rb_data_type_t);

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
    size_of::<nupur_sys::rb_data_type_struct__bindgen_ty_1>() / size_of::<usize>();
const _: () = assert!(
    FUNCTION_TABLE_WORDS == 5 || FUNCTION_TABLE_WORDS == 12,
    "rb_data_type_t's callback table has a layout nupur has not read: check Ruby's \
     include/ruby/internal/core/rtypeddata.h, then list its size here"
);

impl DataType {
    /// `parent` is null for a base type.
    pub const fn new(
        name: *const core::ffi::c_char,
        parent: *const nupur_sys::rb_data_type_t,
        dmark: nupur_sys::RUBY_DATA_FUNC,
        dfree: nupur_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> nupur_sys::size_t>,
        embedded: bool,
    ) -> DataType {
        const FREE_IMMEDIATELY: VALUE =
            nupur_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE;
        const EMBEDDABLE: VALUE = nupur_sys::rbimpl_typeddata_flags::RUBY_TYPED_EMBEDDABLE as VALUE;
        /* Zero first, then set only the fields used: Ruby's partial-initialiser
         * convention (`reserved` "must be filled with zeros"), and what keeps
         * this compiling when Ruby adds a callback to the table, as 4.1 did
         * (`handle_weak_references`, `reserved[1]` -> `reserved[7]`). The
         * layout check above is what turns a layout nobody has read into a
         * build failure, not a silent one. */
        // SAFETY: `rb_data_type_t` is plain C data - pointers, options of
        // function pointers and an integer - for which all-zero is valid, and
        // is Ruby's own initial state for the fields not set below.
        let mut t: nupur_sys::rb_data_type_t = unsafe { core::mem::zeroed() };
        t.wrap_struct_name = name;
        t.function.dmark = dmark;
        t.function.dfree = dfree;
        t.function.dsize = dsize;
        /* No compaction callback (`dcompact` stays None): an object's only
         * values are its slots, marked pinned. */
        t.parent = parent;
        t.data = core::ptr::null_mut();
        /* Never `RUBY_TYPED_FROZEN_SHAREABLE`: an object made shareable could
         * be read from two Ractors at once, and a payload's cells (a borrow
         * count, a cache) are not atomic. Without it Ruby refuses to make one
         * shareable. */
        t.flags = if embedded {
            FREE_IMMEDIATELY | EMBEDDABLE
        } else {
            FREE_IMMEDIATELY
        };
        DataType(t)
    }

    /// The raw pointer the Ruby API wants.
    #[inline]
    pub const fn as_ptr(&self) -> *const nupur_sys::rb_data_type_t {
        self as *const DataType as *const nupur_sys::rb_data_type_t
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
pub(super) fn typed_data(v: Value, ty: &'static DataType) -> Result<*mut c_void, Error> {
    let (sv, ty) = (v.as_sys(), ty.as_ptr());
    // SAFETY: `v` is a live value and `ty` a registered data type; the data
    // pointer is read only once the type is known to match.
    unsafe {
        if nupur_sys::read::rb_typeddata_is_kind_of(sv, ty) {
            let p = data_pointer(sv);
            if p.is_null() {
                return Err(crate::rb::error::type_error(
                    "an object whose data was never allocated",
                ));
            }
            return Ok(p);
        }
        match crate::raw::check_typeddata(sv, ty) {
            Err(e) => Err(e),
            /* rb_typeddata_is_kind_of said no, so the check should have raised. */
            Ok(_) => Err(crate::rb::error::type_error("wrong argument type")),
        }
    }
}

/// The data pointer of a TypedData object, read without a check.
///
/// # Safety
/// `v` is a TypedData object.
#[inline]
pub(super) unsafe fn data_pointer(v: nupur_sys::value::Value) -> *mut c_void {
    // SAFETY: the caller's contract.
    unsafe { nupur_sys::read::rtypeddata_get_data(v) }
}

/// The data pointer of a TypedData object whose type the caller has already
/// established - a receiver its `TryConvert` checked, or the owner a
/// checked node holds.
///
/// A mismatch here is a bug in that reasoning, not a user error, so it panics:
/// the panic unwinds through the Rust frames (running their destructors) and
/// the method's trampoline makes it the internal error (`rb::error::INTERNAL_ERROR`), where a raise
/// would longjmp past them.
pub(super) fn typed_data_known(v: Value, ty: &'static DataType) -> *mut c_void {
    let (sv, ty) = (v.as_sys(), ty.as_ptr());
    // SAFETY: as in `typed_data`; the assert is the type check.
    unsafe {
        assert!(
            nupur_sys::read::rb_typeddata_is_kind_of(sv, ty),
            "a VALUE of an established type had a different one"
        );
        let p = data_pointer(sv);
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
pub(super) unsafe fn wrap_built<T, const N: usize>(
    klass: nupur_sys::value::Value,
    ty: *const rb_data_type_t,
    data: T,
    roots: [nupur_sys::value::Root; N],
) -> Result<nupur_sys::value::Value, Error> {
    const { assert!(core::mem::size_of::<Envelope<T, N>>() != 0) };
    const { assert!(core::mem::align_of::<Envelope<T, N>>() <= 16) }; /* malloc's alignment */
    let obj = crate::raw::typed_object_zalloc(klass, core::mem::size_of::<Envelope<T, N>>(), ty)?;
    let p = data_pointer(obj) as *mut Envelope<T, N>;
    if p.is_null() {
        return Err(crate::rb::error::internal_error(
            "a TypedData object was made without its data",
        ));
    }
    /* The roots are read here, after the allocation - the one point a
     * collection can run - so they stayed on this frame across it; from
     * here the mark sees them in the slots. */
    p.write(Envelope { roots, data });
    Ok(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn mark(_: *mut c_void) {}
    unsafe extern "C" fn free(_: *mut c_void) {}
    unsafe extern "C" fn size(_: *const c_void) -> nupur_sys::size_t {
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

    /// Neither kind of type is one Ruby may make shareable.
    #[test]
    fn no_type_is_frozen_shareable() {
        const SHAREABLE: VALUE =
            nupur_sys::rbimpl_typeddata_flags::RUBY_TYPED_FROZEN_SHAREABLE as VALUE;
        for embedded in [false, true] {
            let ty = DataType::new(
                c"nupur::test".as_ptr(),
                core::ptr::null(),
                Some(mark),
                Some(free),
                Some(size),
                embedded,
            );
            assert_eq!(ty.0.flags & SHAREABLE, 0);
        }
    }

    const FREE_IMMEDIATELY_FLAG: u32 =
        nupur_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as u32;
}
