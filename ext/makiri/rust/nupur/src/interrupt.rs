//! The pending-interrupt check an engine's budgets ask
//! (`rb_thread_interrupted`), handed out for a token, as [`crate::stack`]'s is.

#![allow(unsafe_code)]

use crate::rb::Ruby;

/// "Does the current Ruby thread have an interrupt waiting?" as a plain function
/// pointer, for an engine that knows nothing of Ruby to ask from its budgets.
///
/// Handed out only for a token: the function reads the current Ruby thread's
/// state, so it is for code that runs where the token came from - a registered
/// method, with the GVL held - which is where the extension's XPath and CSS
/// engines run (neither does in a GVL-released body). It only asks. Delivering
/// the interrupt is `raw::thread_check_ints`, which the caller makes once the
/// engine has stopped and dropped what it holds.
pub fn interrupt_probe(_: &Ruby) -> fn() -> bool {
    interrupted
}

#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "private: handed out only by `interrupt_probe`, for a token"
    )
)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        discarded_ruby_error,
        reason = "not dropped: a failed ask is answered `true`, so the caller delivers (`rb_thread_check_ints`) and whatever raised comes out there"
    )
)]
fn interrupted() -> bool {
    /* The pointer outlives the token it was handed out for, so a thread Ruby
     * does not run may call it: it has no thread to ask about, and answers "no
     * interrupt", as a build with no probe does. */
    // SAFETY: `ruby_native_thread_p` reads only the calling thread's own
    // thread-local state, from any thread; a Ruby thread holds the GVL where the
    // engine runs, which is the token's contract.
    unsafe {
        if rb_sys::ruby_native_thread_p() == 0 {
            return false;
        }
        let ruby = Ruby::get_unchecked();
        /* A failed ask is answered "yes": the caller then delivers, and what
         * raised comes out there instead of being lost here. */
        crate::raw::thread_interrupted(&ruby).unwrap_or(true)
    }
}
