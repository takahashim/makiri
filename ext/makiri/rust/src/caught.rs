//! Catching a panic at a C boundary, and re-raising it once past one.
//!
//! The crate unwinds, so a panic normally becomes a Ruby exception on the
//! thread that raised it. That stops being true the moment a C frame is in the
//! way: Rust turns an unwind at an `extern "C"` boundary into an ABORT, because
//! unwinding through frames compiled without unwind tables is undefined
//! behaviour. Lexbor calls back into us in a dozen places - the CSS traversal,
//! the serializers, the tokenizer's token-done hook - and so does Ruby, through
//! `rb_protect` and `rb_thread_call_without_gvl`. A panic in any of those would
//! take the host down, which is exactly what `panic = "unwind"` exists to stop.
//!
//! So a callback does not panic INTO C. It catches the panic, latches it, and
//! returns the status that asks C to stop. The caller, once C has unwound
//! normally and every resource it owns is released, calls [`PanicLatch::resume`]
//! and the panic continues from a frame that has only Rust above it.
//!
//! Deferred, not swallowed: `resume_unwind` re-raises the ORIGINAL payload, so
//! the message and the exception Ruby finally sees are the ones the panic
//! carried. The only difference from panicking directly is that the C frames
//! were unwound by C, in the way C expects.
//!
//! This is the same shape the callbacks already use for the node cap and for an
//! allocation failure - latch a flag, return STOP, report after the walk - and
//! it is deliberately the same, because a panic has no better claim than an OOM
//! to be handled mid-traversal.

use core::any::Any;
use core::panic::AssertUnwindSafe;

/// A caught panic, waiting for a frame where re-raising it is safe.
#[derive(Default)]
pub struct PanicLatch(Option<Box<dyn Any + Send + 'static>>);

impl PanicLatch {
    pub const fn new() -> PanicLatch {
        PanicLatch(None)
    }

    /// Whether a panic has been caught. Callers test this the way they test
    /// their `oom` flag: it means "stop, the result is not usable".
    #[inline]
    pub fn caught(&self) -> bool {
        self.0.is_some()
    }

    /// Run `f`; if it panics, latch the panic and return `stop` instead.
    ///
    /// The FIRST panic is the one kept. A traversal can call back many times,
    /// and the later ones are consequences of stopping, not new information.
    #[inline]
    pub fn guard<R>(&mut self, stop: R, f: impl FnOnce() -> R) -> R {
        match std::panic::catch_unwind(AssertUnwindSafe(f)) {
            Ok(v) => v,
            Err(payload) => {
                if self.0.is_none() {
                    self.0 = Some(payload);
                }
                stop
            }
        }
    }

    /// Re-raise the latched panic, if there is one.
    ///
    /// Call this only where no C frame is left on the stack - that is the whole
    /// point - and after anything the interrupted work owned has been released.
    #[inline]
    pub fn resume(&mut self) {
        if let Some(payload) = self.0.take() {
            std::panic::resume_unwind(payload);
        }
    }
}

/// The message a panic payload carries, as `panic!` and the standard library's
/// own panics produce it. `"a panic"` when it is neither of the two shapes the
/// runtime uses, which no panic in this crate is.
pub fn message(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "a panic"
    }
}
