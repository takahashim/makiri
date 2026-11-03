//! The layer's bodies that must not unwind, and the build that checks they
//! cannot (`rake no_unwind`): the guard is `nupur_sys`'s, where the protect
//! thunk runs under it; the sites below are Nupur's own, which that crate
//! does not name.

#[cfg(nupur_no_unwind)]
pub use nupur_sys::no_unwind::self_test;
pub use nupur_sys::no_unwind::{must_not_unwind, Site};

nupur_sys::no_unwind_sites! {
    /// `typed`'s mark callback.
    GcMark => "nupur_unwinds_gc_mark",
    /// `typed`'s free callback: the data's whole drop glue.
    GcFree => "nupur_unwinds_gc_free",
    /// `typed`'s free callback for an embedded type.
    GcFreeEmbedded => "nupur_unwinds_gc_free_embedded",
    /// `typed`'s memsize callback.
    GcMemsize => "nupur_unwinds_gc_memsize",
    /// `raw::hash_foreach`'s `rb_hash_foreach` callback.
    HashEach => "nupur_unwinds_hash_each",
    /// `gvl::without_gvl`'s body, run with the GVL released.
    GvlBody => "nupur_unwinds_gvl_body",
    /// `gvl::without_gvl_cancellable`'s unblock function, which Ruby calls
    /// from another thread to ask the body to stop.
    GvlUnblock => "nupur_unwinds_gvl_unblock",
    /// A method's trampoline (`rb::method::finish`, which `finish_cary` runs
    /// too).
    Method => "nupur_unwinds_method",
    /// The extension's `Init_` (`rb::method::run_init`).
    Init => "nupur_unwinds_init",
    /// `string::utf8_str_filled`'s guard, which empties a String a panic left
    /// unfilled - while that panic unwinds.
    DropUnfilled => "nupur_unwinds_drop_unfilled",
    /// `rb::kept::Kept`'s `Drop`, which hands its value to `keep_alive`.
    DropKept => "nupur_unwinds_drop_kept",
    /// `RString::with_locked_bytes`'s guard, which unlocks the String - while
    /// a panic unwinds, too.
    DropUnlock => "nupur_unwinds_drop_unlock",
}
