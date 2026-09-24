//! Lexbor's serializer callback, written once. Every Lexbor serializer streams
//! its output as `(data, len, ctx)` chunks; [`chunk_cb`] appends them to a
//! [`Chunks`] - whatever buffer the caller collects into - and never lets a
//! panic unwind into C.
//!
//! The HTML serializer collects into a capped [`Buf`], the stylesheet reader
//! into a reused `Vec`; the callback, its status mapping and its panic latch
//! were two copies of the same `extern "C"` function until they were this one.

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::caught::PanicLatch;
use crate::cbuf::Buf;
use crate::falloc::VecPush;
use crate::lexbor::abi::consts as k;

/// Something serializer output is appended to. `false` refuses the chunk -
/// a ceiling or an allocation failure - and stops the serializer.
pub trait ChunkSink {
    fn take(&mut self, bytes: &[u8]) -> bool;
}

impl ChunkSink for Buf {
    fn take(&mut self, bytes: &[u8]) -> bool {
        self.append(bytes).is_ok()
    }
}

impl ChunkSink for Vec<u8> {
    fn take(&mut self, bytes: &[u8]) -> bool {
        self.falloc_extend(bytes).is_ok()
    }
}

/// A sink plus what the callback reports beside the bytes: whether it refused
/// a chunk, and a panic caught on the way.
pub struct Chunks<S> {
    pub sink: S,
    /// Set when the sink refused a chunk; the serializer was stopped there.
    pub refused: bool,
    /// A panic in the append, latched: the callback is called from C, and
    /// unwinding into Lexbor aborts. The caller re-raises it once Lexbor has
    /// returned.
    pub panic: PanicLatch,
}

impl<S: ChunkSink> Chunks<S> {
    pub fn new(sink: S) -> Self {
        Chunks {
            sink,
            refused: false,
            panic: PanicLatch::new(),
        }
    }

    /// The `ctx` to hand Lexbor beside [`chunk_cb::<S>`](chunk_cb).
    pub fn ctx(&mut self) -> *mut c_void {
        self as *mut Chunks<S> as *mut c_void
    }
}

/// Lexbor's chunk sink, for a `ctx` from [`Chunks::ctx`] of the same `S`.
///
/// # Safety
/// `ctx` is a live `Chunks<S>` nothing else borrows for the call, and `data`
/// names `len` readable bytes (or is null with `len` 0) - Lexbor's contract.
pub unsafe extern "C" fn chunk_cb<S: ChunkSink>(
    data: *const u8,
    len: usize,
    ctx: *mut c_void,
) -> u32 {
    let c = &mut *(ctx as *mut Chunks<S>);
    let (sink, refused) = (&mut c.sink, &mut c.refused);
    c.panic.guard(k::STATUS_ERROR_MEMORY_ALLOCATION, || {
        if len == 0 {
            return k::STATUS_OK;
        }
        let taken = !data.is_null() && {
            // SAFETY: Lexbor hands `len` readable bytes at `data`.
            sink.take(unsafe { core::slice::from_raw_parts(data, len) })
        };
        if taken {
            k::STATUS_OK
        } else {
            *refused = true;
            k::STATUS_ERROR_MEMORY_ALLOCATION
        }
    })
}
