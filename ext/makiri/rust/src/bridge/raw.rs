//! The Ruby C calls that can RAISE - every `longjmp` this extension can start.
//!
//! A raise is `longjmp`, which crosses Rust frames without running their
//! destructors. So a call that can raise is made either under
//! [`protect`](super::ruby::protect) (the raise comes back as `Err`) or from a
//! frame that owns nothing a skipped `Drop` would leak. Which calls those are
//! is the whole question, and this file is the answer: each wrapper names the
//! C function and says what raises it. `rake unsafe:boundaries` pins that
//! these C names appear nowhere else in the crate, so a new raising call is a
//! new wrapper here, with its raise stated.
//!
//! Every wrapper is `unsafe`, for the same reason: the caller is the one who
//! knows whether a destructor is live. They are thin - one C call each, no
//! Rust that can panic - so the two raw `rb_protect` thunks that must contain
//! only C calls (`ruby::exception_message_thunk`,
//! `xml_decode::strict_transcode_thunk`) can use them too.
//!
//! What is NOT here: calls that cannot raise (`rb_gc_mark`, the encoding
//! singletons, `rb_typeddata_is_kind_of`, `ruby_xfree`), the GC callbacks,
//! and the calls magnus makes on the bridge's behalf (`Ruby::ary_new`,
//! `float_from_f64`, `funcall`), which raise by magnus's own rules.

#![allow(unsafe_code)]
/* Every wrapper's contract is the module's, stated once above. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long, c_void};

use rb_sys::{rb_data_type_t, rb_encoding, ID, VALUE};

/// The raise sweep's hook (`falloc::inject`, `script/check_raise_injection.rb`):
/// in an `alloc-inject` build the armed allocating call below raises
/// `NoMemoryError` in place of its allocation, as a real failure would - so
/// the sweep can make every one of them raise in turn. Nothing in any other
/// build.
#[inline(always)]
unsafe fn injected_raise() {
    #[cfg(feature = "alloc-inject")]
    if crate::falloc::raise_inject_should_raise() {
        /* Raises without allocating: Ruby keeps the exception preallocated. */
        unsafe { rb_sys::rb_memerror() }
    }
}

/* ---- conversions and calls into Ruby code ---- */

/// `rb_String`: raises `TypeError`, or whatever the value's `to_str`/`to_s`
/// raises.
#[inline]
pub unsafe fn string(v: VALUE) -> VALUE {
    unsafe { rb_sys::rb_String(v) }
}

/// `rb_funcallv`: raises whatever the method raises (`NoMethodError`
/// included).
#[inline]
pub unsafe fn funcallv(recv: VALUE, method: ID, args: &[VALUE]) -> VALUE {
    unsafe { rb_sys::rb_funcallv(recv, method, args.len() as c_int, args.as_ptr()) }
}

/// `rb_funcall` with no arguments: as [`funcallv`].
#[inline]
pub unsafe fn funcall0(recv: VALUE, method: ID) -> VALUE {
    unsafe { rb_sys::rb_funcall(recv, method, 0) }
}

/// `rb_obj_as_string`: raises whatever the value's `to_s` raises.
#[inline]
pub unsafe fn obj_as_string(v: VALUE) -> VALUE {
    unsafe { rb_sys::rb_obj_as_string(v) }
}

/// `rb_respond_to`: raises whatever the object's `respond_to?` (or
/// `respond_to_missing?`) raises.
#[inline]
pub unsafe fn respond_to(v: VALUE, method: ID) -> bool {
    unsafe { rb_sys::rb_respond_to(v, method) != 0 }
}

/// `rb_check_frozen`: raises `FrozenError` for a frozen value.
#[inline]
pub unsafe fn check_frozen(v: VALUE) {
    unsafe { rb_sys::rb_check_frozen(v) }
}

/// `rb_range_beg_len`: raises `RangeError` for a bound too large for a
/// `long` (with `err = 0`, a start outside the collection is `Qnil`, not a
/// raise).
#[inline]
pub unsafe fn range_beg_len(
    range: VALUE,
    beg: &mut c_long,
    len: &mut c_long,
    count: c_long,
    err: c_int,
) -> VALUE {
    unsafe { rb_sys::rb_range_beg_len(range, beg, len, count, err) }
}

/// `rb_thread_check_ints`: raises a pending interrupt (`Interrupt`,
/// `Thread#raise`'s exception, a `Timeout`).
#[inline]
pub unsafe fn thread_check_ints() {
    unsafe { rb_sys::rb_thread_check_ints() }
}

/* ---- symbols ---- */

/// `rb_intern3`: raises `NoMemoryError` when the symbol table cannot grow
/// (interning valid UTF-8 raises nothing else).
#[inline]
pub unsafe fn intern3(name: *const c_char, len: c_long, enc: *mut rb_encoding) -> ID {
    unsafe {
        injected_raise();
        rb_sys::rb_intern3(name, len, enc)
    }
}

/// `rb_intern` of a NUL-terminated name: as [`intern3`].
#[inline]
pub unsafe fn intern_cstr(name: *const c_char) -> ID {
    unsafe {
        injected_raise();
        rb_sys::rb_intern(name)
    }
}

/* ---- Strings ---- */

/// `rb_utf8_str_new`: raises `NoMemoryError`.
#[inline]
pub unsafe fn utf8_str_new(ptr: *const c_char, len: c_long) -> VALUE {
    unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(ptr, len)
    }
}

/// `rb_enc_interned_str`: raises `NoMemoryError`.
#[inline]
pub unsafe fn enc_interned_str(ptr: *const c_char, len: c_long, enc: *mut rb_encoding) -> VALUE {
    unsafe {
        injected_raise();
        rb_sys::rb_enc_interned_str(ptr, len, enc)
    }
}

/// `rb_str_dup`: raises `NoMemoryError`.
#[inline]
pub unsafe fn str_dup(s: VALUE) -> VALUE {
    unsafe {
        injected_raise();
        rb_sys::rb_str_dup(s)
    }
}

/// `rb_str_subseq`: raises `NoMemoryError`.
#[inline]
pub unsafe fn str_subseq(s: VALUE, beg: c_long, len: c_long) -> VALUE {
    unsafe {
        injected_raise();
        rb_sys::rb_str_subseq(s, beg, len)
    }
}

/// `rb_enc_associate`: raises `FrozenError` for a frozen String, and
/// `ArgumentError` for an encoding the String's bytes cannot carry.
#[inline]
pub unsafe fn enc_associate(s: VALUE, enc: *mut rb_encoding) -> VALUE {
    unsafe { rb_sys::rb_enc_associate(s, enc) }
}

/// `rb_str_encode` to `enc` with `flags`: raises `Encoding::ConverterNotFoundError`
/// when Ruby has no converter, and - without replacement flags -
/// `Encoding::InvalidByteSequenceError` / `UndefinedConversionError`.
#[inline]
pub unsafe fn str_encode(s: VALUE, enc: *mut rb_encoding, flags: c_int) -> VALUE {
    unsafe {
        rb_sys::rb_str_encode(
            s,
            rb_sys::rb_enc_from_encoding(enc),
            flags,
            rb_sys::Qnil as VALUE,
        )
    }
}

/// `rb_to_encoding`: raises `ArgumentError` for an unknown name and
/// `TypeError` for a value that is neither a String nor an `Encoding`.
#[inline]
pub unsafe fn to_encoding(v: VALUE) -> *mut rb_encoding {
    unsafe { rb_sys::rb_to_encoding(v) }
}

/// `rb_enc_find_index` + `rb_enc_from_index`: may autoload the encoding,
/// which can raise (`LoadError`) and is a GC point; `null` for a name Ruby
/// does not know. Not `rb_enc_find`, which answers ASCII-8BIT for one.
#[inline]
pub unsafe fn enc_find(name: *const c_char) -> *mut rb_encoding {
    unsafe {
        let index = rb_sys::rb_enc_find_index(name);
        if index < 0 {
            core::ptr::null_mut()
        } else {
            rb_sys::rb_enc_from_index(index)
        }
    }
}

/* ---- typed data and Ruby's allocator ---- */

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub unsafe fn check_typeddata(v: VALUE, ty: *const rb_data_type_t) -> *mut c_void {
    unsafe { rb_sys::rb_check_typeddata(v, ty) }
}

/// `rb_data_typed_object_wrap`: raises `NoMemoryError`.
#[inline]
pub unsafe fn typed_object_wrap(
    klass: VALUE,
    data: *mut c_void,
    ty: *const rb_data_type_t,
) -> VALUE {
    unsafe {
        injected_raise();
        rb_sys::rb_data_typed_object_wrap(klass, data, ty)
    }
}

/// `ruby_xmalloc`: raises `NoMemoryError`.
#[inline]
pub unsafe fn xmalloc(size: usize) -> *mut c_void {
    unsafe {
        injected_raise();
        rb_sys::ruby_xmalloc(size as rb_sys::size_t)
    }
}

/// `ruby_xrealloc2`: raises `NoMemoryError`, and `ArgumentError` when
/// `n * size` overflows.
#[inline]
pub unsafe fn xrealloc2(p: *mut c_void, n: usize, size: usize) -> *mut c_void {
    unsafe {
        injected_raise();
        rb_sys::ruby_xrealloc2(p, n as rb_sys::size_t, size as rb_sys::size_t)
    }
}
