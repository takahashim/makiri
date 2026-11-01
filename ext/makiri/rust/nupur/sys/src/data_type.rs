//! A TypedData type's `rb_data_type_t`, built where its layout is known:
//! the callbacks, the flags (`VALUE`-typed bits, which only this crate
//! spells), and the check that the callback table is a layout Nupur has
//! read.

#![allow(unsafe_code)]

use rb_sys::{rb_data_type_t, VALUE};

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; these are set at
/// compile time and never written. `repr(transparent)` keeps the layout exactly
/// `rb_data_type_t`, which is what `rb_data_typed_object_wrap` reads.
#[repr(transparent)]
pub struct DataType(rb_data_type_t);

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

/// What a type's objects are, for Ruby: the flags of its `rb_data_type_t`,
/// as the choices a type has rather than bits.
///
/// Every type is freed at once when its object is collected
/// (`RUBY_TYPED_FREE_IMMEDIATELY`: the free callback only drops and frees,
/// which needs no deferral). What a type chooses is whether its data may be
/// embedded in the object's own slot (`RUBY_TYPED_EMBEDDABLE`). And no type
/// may be made shareable (`RUBY_TYPED_FROZEN_SHAREABLE` is not among the
/// choices): an object made shareable could be read from two Ractors at
/// once, and a payload's cells (a borrow count, a cache) are not atomic.
/// Without the flag Ruby refuses to make one shareable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DataFlags {
    embeddable: bool,
}

impl DataFlags {
    /// Data apart from the object, in a block of its own.
    pub const SEPARATE: DataFlags = DataFlags { embeddable: false };
    /// Data that Ruby may embed in the object's own slot.
    pub const EMBEDDABLE: DataFlags = DataFlags { embeddable: true };

    /// The `flags` word, as `rb_data_type_t` holds it.
    const fn bits(self) -> VALUE {
        const FREE_IMMEDIATELY: VALUE =
            rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE;
        const EMBEDDABLE: VALUE = rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_EMBEDDABLE as VALUE;
        if self.embeddable {
            FREE_IMMEDIATELY | EMBEDDABLE
        } else {
            FREE_IMMEDIATELY
        }
    }
}

impl DataType {
    /// A type named `name` (what Ruby calls it in its messages and
    /// `ObjectSpace`), deriving from `parent` (`None` for a base type), with
    /// its GC callbacks and its [`DataFlags`].
    pub const fn new(
        name: &'static core::ffi::CStr,
        parent: Option<&'static DataType>,
        dmark: rb_sys::RUBY_DATA_FUNC,
        dfree: rb_sys::RUBY_DATA_FUNC,
        dsize: Option<unsafe extern "C" fn(*const core::ffi::c_void) -> rb_sys::size_t>,
        flags: DataFlags,
    ) -> DataType {
        /* Zero first, then set only the fields used: Ruby's partial-initialiser
         * convention (`reserved` "must be filled with zeros"), and what keeps
         * this compiling when Ruby adds a callback to the table, as 4.1 did
         * (`handle_weak_references`, `reserved[1]` -> `reserved[7]`). The
         * layout check above is what turns a layout nobody has read into a
         * build failure, not a silent one. */
        // SAFETY: `rb_data_type_t` is plain C data - pointers, options of
        // function pointers and an integer - for which all-zero is valid, and
        // is Ruby's own initial state for the fields not set below.
        let mut t: rb_data_type_t = unsafe { core::mem::zeroed() };
        t.wrap_struct_name = name.as_ptr();
        t.function.dmark = dmark;
        t.function.dfree = dfree;
        t.function.dsize = dsize;
        /* No compaction callback (`dcompact` stays None): an object's only
         * values are its slots, marked pinned. */
        t.parent = match parent {
            Some(p) => p.as_ptr(),
            None => core::ptr::null(),
        };
        t.data = core::ptr::null_mut();
        t.flags = flags.bits();
        DataType(t)
    }

    /// The raw pointer the Ruby API wants - this crate's alone: its
    /// wrappers take `&DataType`.
    #[inline]
    pub(crate) const fn as_ptr(&self) -> *const rb_data_type_t {
        self as *const DataType as *const rb_data_type_t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;

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
            c"nupur::test",
            None,
            Some(mark),
            Some(free),
            Some(size),
            DataFlags::SEPARATE,
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
            rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FROZEN_SHAREABLE as VALUE;
        for flags in [DataFlags::SEPARATE, DataFlags::EMBEDDABLE] {
            let ty = DataType::new(
                c"nupur::test",
                None,
                Some(mark),
                Some(free),
                Some(size),
                flags,
            );
            assert_eq!(ty.0.flags & SHAREABLE, 0);
        }
    }

    const FREE_IMMEDIATELY_FLAG: u32 =
        rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as u32;
}
