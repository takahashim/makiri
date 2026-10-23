//! The Ruby C calls that can RAISE - every `longjmp` this extension can start -
//! and each one made under `rb_protect`, so the raise comes back as `Err`.
//!
//! A raise is `longjmp`, which crosses Rust frames without running their
//! destructors, and crossing a frame that has one (or a `catch_unwind`) is
//! undefined behaviour. So no raising call is made anywhere but here, and here
//! every one runs inside [`call`]: a bare `rb_protect` whose `extern "C"`
//! thunk holds nothing but that one C call and the `Copy` values it reads. The
//! only frame a raise crosses is the thunk's own, which owns nothing. A
//! caller gets a `Result` and owns whatever it likes across the call.
//! `rake unsafe:boundaries` pins that these C names appear nowhere else in the
//! crate, so a new raising call is a new wrapper here, with its raise stated.
//!
//! Every wrapper is `unsafe` for its arguments alone: a `VALUE` must be live,
//! a pointer valid for what the C function reads or writes.
//!
//! What is NOT here: calls that cannot raise (`rb_gc_mark`, the encoding
//! singletons, `rb_typeddata_is_kind_of`, `rb_attr_get`, `ruby_xfree`), the GC
//! callbacks, and the calls magnus makes on the bridge's behalf, which raise by
//! magnus's own rules (`bridge::build` is the bridge's own spelling of those).

#![allow(unsafe_code)]
/* Every wrapper's contract is the module's, stated once above. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long, c_void};

use magnus::Error;
use rb_sys::{rb_data_type_t, rb_encoding, ID, VALUE};

/// The raise sweep's hook (`falloc::inject`, `script/check_raise_injection.rb`):
/// in an `alloc-inject` build the armed allocating call below raises
/// `NoMemoryError` in place of its allocation, as a real failure would - so
/// the sweep can make every one of them raise in turn. Nothing in any other
/// build. Called inside the thunk, so the raise is protected like the call's.
#[inline(always)]
unsafe fn injected_raise() {
    #[cfg(feature = "alloc-inject")]
    if crate::falloc::raise_inject_should_raise() {
        /* Raises without allocating: Ruby keeps the exception preallocated. */
        unsafe { rb_sys::rb_memerror() }
    }
}

/// Run `f` - one C call, and the `Copy` values it reads - under a bare
/// `rb_protect`; a raise comes back as `Err`.
///
/// `F: Copy` and `T: Copy` are what keep the thunk's frame free of anything a
/// skipped destructor would leak: a closure is `Copy` only when everything it
/// captures is, so it owns nothing, and neither does its result. There is no
/// `catch_unwind` here (a raise crossing one is undefined behaviour); a panic
/// in the thunk would abort at its `extern "C"` boundary, and the closures
/// below hold no Rust that panics.
#[inline]
fn call<T: Copy, F: FnOnce() -> T + Copy>(f: F) -> Result<T, Error> {
    struct Slot<F, T> {
        f: F,
        out: Option<T>,
    }
    unsafe extern "C" fn thunk<T: Copy, F: FnOnce() -> T + Copy>(arg: VALUE) -> VALUE {
        // SAFETY: `arg` is the `Slot` `call` passed, live for the call.
        let slot = unsafe { &mut *(arg as *mut Slot<F, T>) };
        slot.out = Some((slot.f)());
        rb_sys::Qnil as VALUE
    }
    let mut slot = Slot { f, out: None };
    let mut state: c_int = 0;
    // SAFETY: the thunk reads only `slot`, which outlives the call.
    unsafe {
        rb_sys::rb_protect(
            Some(thunk::<T, F>),
            &mut slot as *mut Slot<F, T> as VALUE,
            &mut state,
        );
    }
    match (state, slot.out) {
        (0, Some(out)) => Ok(out),
        (0, None) => Err(crate::bridge::ruby::internal_error(
            "a protected call did not run",
        )),
        (state, _) => Err(jump_error(state)),
    }
}

/// The error a non-zero `rb_protect` state stands for, as magnus makes it:
/// the state is resumed under magnus's own protect, which answers it - an
/// exception for a raise, the tag for a `throw` or a thread kill. The closure
/// holds only the C call.
#[cold]
fn jump_error(state: c_int) -> Error {
    // SAFETY: `state` is the one `rb_protect` just reported, its error info
    // still set; resuming it is what magnus's protect catches.
    match magnus::rb_sys::protect(|| -> VALUE { unsafe { rb_sys::rb_jump_tag(state) } }) {
        Err(e) => e,
        Ok(_) => crate::bridge::ruby::internal_error("rb_jump_tag returned"),
    }
}

/* ---- conversions ---- */

/// `rb_sym2str`: raises `NoMemoryError` when a static Symbol's name String is
/// made for the first time. Runs no Ruby code.
#[inline]
pub unsafe fn sym2str(v: VALUE) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_sym2str(v)
    })
}

/// `rb_check_frozen`: raises `FrozenError` for a frozen value.
#[inline]
pub unsafe fn check_frozen(v: VALUE) -> Result<(), Error> {
    call(|| unsafe { rb_sys::rb_check_frozen(v) })
}

/// `rb_range_beg_len` with `err = 0`: raises `RangeError` for a bound too
/// large for a `long` (a start outside the collection is `Qnil`, not a
/// raise). `beg` and `len` are written on success.
#[inline]
pub unsafe fn range_beg_len(
    range: VALUE,
    beg: *mut c_long,
    len: *mut c_long,
    count: c_long,
) -> Result<VALUE, Error> {
    call(|| unsafe { rb_sys::rb_range_beg_len(range, beg, len, count, 0) })
}

/// `rb_thread_check_ints`: raises a pending interrupt (`Interrupt`,
/// `Thread#raise`'s exception, a `Timeout`), or ends the thread.
#[inline]
pub unsafe fn thread_check_ints() -> Result<(), Error> {
    call(|| unsafe { rb_sys::rb_thread_check_ints() })
}

/* ---- symbols ---- */

/// `rb_intern3`: raises `NoMemoryError` when the symbol table cannot grow
/// (interning valid UTF-8 raises nothing else).
#[inline]
pub unsafe fn intern3(
    name: *const c_char,
    len: c_long,
    enc: *mut rb_encoding,
) -> Result<ID, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern3(name, len, enc)
    })
}

/// `rb_intern` of a NUL-terminated name: as [`intern3`].
#[inline]
pub unsafe fn intern_cstr(name: *const c_char) -> Result<ID, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern(name)
    })
}

/* ---- Strings ---- */

/// `rb_utf8_str_new`: raises `NoMemoryError`.
#[inline]
pub unsafe fn utf8_str_new(ptr: *const c_char, len: c_long) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(ptr, len)
    })
}

/// `rb_enc_interned_str`: raises `NoMemoryError`.
#[inline]
pub unsafe fn enc_interned_str(
    ptr: *const c_char,
    len: c_long,
    enc: *mut rb_encoding,
) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_enc_interned_str(ptr, len, enc)
    })
}

/// `rb_str_dup`: raises `NoMemoryError`.
#[inline]
pub unsafe fn str_dup(s: VALUE) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_str_dup(s)
    })
}

/// `rb_str_subseq`: raises `NoMemoryError`.
#[inline]
pub unsafe fn str_subseq(s: VALUE, beg: c_long, len: c_long) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_str_subseq(s, beg, len)
    })
}

/// `rb_enc_associate`: raises `FrozenError` for a frozen String, and
/// `ArgumentError` for an encoding the String's bytes cannot carry.
#[inline]
pub unsafe fn enc_associate(s: VALUE, enc: *mut rb_encoding) -> Result<VALUE, Error> {
    call(|| unsafe { rb_sys::rb_enc_associate(s, enc) })
}

/// `rb_str_encode` to `enc` with `flags`: raises `Encoding::ConverterNotFoundError`
/// when Ruby has no converter, and - without replacement flags -
/// `Encoding::InvalidByteSequenceError` / `UndefinedConversionError`.
#[inline]
pub unsafe fn str_encode(s: VALUE, enc: *mut rb_encoding, flags: c_int) -> Result<VALUE, Error> {
    call(|| unsafe {
        rb_sys::rb_str_encode(
            s,
            rb_sys::rb_enc_from_encoding(enc),
            flags,
            rb_sys::Qnil as VALUE,
        )
    })
}

/// `rb_to_encoding`: raises `ArgumentError` for an unknown name and
/// `TypeError` for a value that is neither a String nor an `Encoding`. The
/// caller passes only an `Encoding` or a String, for which it runs no Ruby
/// code (`bridge::string::to_encoding`).
#[inline]
pub unsafe fn to_encoding(v: VALUE) -> Result<*mut rb_encoding, Error> {
    call(|| unsafe { rb_sys::rb_to_encoding(v) })
}

/// `rb_enc_find_index` + `rb_enc_from_index`: may autoload the encoding,
/// which can raise (`LoadError`) and is a GC point; `null` for a name Ruby
/// does not know. Not `rb_enc_find`, which answers ASCII-8BIT for one.
#[inline]
pub unsafe fn enc_find(name: *const c_char) -> Result<*mut rb_encoding, Error> {
    call(|| unsafe {
        let index = rb_sys::rb_enc_find_index(name);
        if index < 0 {
            core::ptr::null_mut()
        } else {
            rb_sys::rb_enc_from_index(index)
        }
    })
}

/* ---- typed data and Ruby's allocator ---- */

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub unsafe fn check_typeddata(v: VALUE, ty: *const rb_data_type_t) -> Result<*mut c_void, Error> {
    call(|| unsafe { rb_sys::rb_check_typeddata(v, ty) })
}

/// `rb_data_typed_object_wrap`: raises `NoMemoryError`.
#[inline]
pub unsafe fn typed_object_wrap(
    klass: VALUE,
    data: *mut c_void,
    ty: *const rb_data_type_t,
) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_data_typed_object_wrap(klass, data, ty)
    })
}

/// `ruby_xmalloc`: raises `NoMemoryError`.
#[inline]
pub unsafe fn xmalloc(size: usize) -> Result<*mut c_void, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::ruby_xmalloc(size as rb_sys::size_t)
    })
}

/// `ruby_xrealloc2`: raises `NoMemoryError`, and `ArgumentError` when
/// `n * size` overflows.
#[inline]
pub unsafe fn xrealloc2(p: *mut c_void, n: usize, size: usize) -> Result<*mut c_void, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::ruby_xrealloc2(p, n as rb_sys::size_t, size as rb_sys::size_t)
    })
}

/* ---- Arrays, Hashes and numbers (`bridge::build` spells these for the glue) ---- */

/// `rb_ary_new_capa`: raises `NoMemoryError`, and `ArgumentError` for a
/// capacity too large.
#[inline]
pub unsafe fn ary_new_capa(capa: c_long) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_new_capa(capa)
    })
}

/// `rb_ary_push`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// Array.
#[inline]
pub unsafe fn ary_push(a: VALUE, v: VALUE) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_push(a, v)
    })
}

/// `rb_hash_new`: raises `NoMemoryError`.
#[inline]
pub unsafe fn hash_new() -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_new()
    })
}

/// `rb_hash_aset`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// Hash. The key is a String or a Symbol, whose hash Ruby computes itself:
/// any other key would have its `#hash` and `#eql?` called.
#[inline]
pub unsafe fn hash_aset(h: VALUE, k: VALUE, v: VALUE) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, k, v)
    })
}

/// `rb_ll2inum`: raises `NoMemoryError` for a value past a Fixnum, which is
/// a Bignum.
#[inline]
pub unsafe fn ll2inum(n: i64) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ll2inum(n)
    })
}

/// `rb_ull2inum`: as [`ll2inum`].
#[inline]
pub unsafe fn ull2inum(n: u64) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ull2inum(n)
    })
}

/// `rb_float_new`: raises `NoMemoryError` for a value that is not a flonum.
#[inline]
pub unsafe fn float_new(f: f64) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_float_new(f)
    })
}

/// `rb_id2sym`: raises `NoMemoryError` when a dynamic Symbol is made.
#[inline]
pub unsafe fn id2sym(id: ID) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_id2sym(id)
    })
}

/// `rb_str_cat`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// String.
#[inline]
pub unsafe fn str_cat(s: VALUE, ptr: *const c_char, len: c_long) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_str_cat(s, ptr, len)
    })
}

/// `rb_enumeratorize_with_size` with no arguments and no size: raises
/// `NoMemoryError`.
#[inline]
pub unsafe fn enumeratorize(obj: VALUE, meth: VALUE) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_enumeratorize_with_size(obj, meth, 0, core::ptr::null(), None)
    })
}
