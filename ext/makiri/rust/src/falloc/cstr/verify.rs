//! Kani proof for the live C-string allocation.
//!
//! `str_alloc` is the hand-written libc allocation behind the typed API, and the
//! terminator it writes is what every caller handing the result on as a C string
//! depends on. That write is the only net for the property: delete it and the
//! Rust selftest stays green and every spec stays green, because a caller writes
//! its `n` bytes and the byte at `n` happens to read as zero - so it is proved
//! here rather than assumed.
//!
//! The dead `reallocarray` / `callocarray` ownership proof and the proof-only
//! `strndup` that used to sit beside this are gone: no live path called them.
//! The live realloc is `cbuf`'s, proved against a shadow model in
//! `cbuf::verify`, and the OOM branches no Kani run reaches are `rake oom`'s.

#![allow(unsafe_code)]
#![cfg(kani)]

use core::ffi::c_void;

use super::str_alloc;

/// `str_alloc` writes the terminator at `n`, or answers NULL.
#[kani::proof]
#[kani::unwind(8)]
fn str_alloc_terminates() {
    let n: usize = kani::any();
    kani::assume(n <= 4);
    unsafe {
        let p = str_alloc(n);
        if !p.is_null() {
            assert!(*p.add(n) == 0, "str_alloc: terminated at n");
            libc_free(p as *mut c_void);
        }
    }
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}
