//! Whether the Ruby thread running a query has an interrupt waiting
//! (`Timeout`, `Thread#raise`, a signal handler, a time-slice switch),
//! asked from the engine's budgets - the same seam as [`crate::stack`].
//!
//! An XPath or CSS evaluation holds the GVL and a read guard of its document
//! from start to end, so none of those could be delivered until it returned.
//! Nothing may yield the GVL in the middle: another thread would then run
//! under the guard (its edit refused where it used to queue), and Ruby code a
//! handler runs could reach anything the evaluation holds. So an evaluation
//! does not check in place. It STOPS - every guard and borrow dropped on the
//! way out, as for any failure - and the Ruby side then delivers the interrupt
//! (`rb_thread_check_ints`) and runs the evaluation again, which is safe
//! because an evaluation only reads. That goes at most [`PASSES`] - 1 times
//! over: a time-slice timer sets the same flag a `Timeout` does and cannot be
//! told from it, so without a bound an evaluation longer than a slice, with
//! another thread running, would be restarted for ever. The last pass is not
//! asked, and is the evaluation as it was before this existed.
//!
//! Budgets ask at chunk boundaries ([`CHUNK`] units of their work), not on
//! every charge, so the per-charge cost is the comparison against a cap that
//! it always was; the cap is only lower by a chunk. This module holds the
//! seam alone: Ruby-free, [`pending`] is a constant `false` where nothing is
//! installed (cargo tests, Kani, the fuzz targets), and so is [`armed`].

#![forbid(unsafe_code)]

use core::cell::Cell;
use std::sync::OnceLock;

/// How many times one query may be run: asked on every pass but the last.
pub const PASSES: usize = 3;

/// Units of a budget's work between two asks. About 1 ms of evaluation at the
/// 50M-op default's measured rate, so a `Timeout` is delivered within a few
/// ms, and a long evaluation asks hundreds of times, not millions.
pub const CHUNK: usize = 1 << 18;

static PROBE: OnceLock<fn() -> bool> = OnceLock::new();

thread_local! {
    /// Whether the query now running on this thread may be stopped to deliver
    /// an interrupt (see [`Armed`]).
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Install the probe. Only `Init_makiri` calls this, once.
pub fn install(probe: fn() -> bool) {
    let _ = PROBE.set(probe);
}

/// Does the current Ruby thread have an interrupt waiting? `false` where no
/// probe is installed. An engine asks only when [`armed`].
pub fn pending() -> bool {
    PROBE.get().is_some_and(|probe| probe())
}

/// May the query being started on this thread be stopped for an interrupt?
/// Read once, when its budget is made.
pub fn armed() -> bool {
    ARMED.with(Cell::get)
}

/// A scope in which queries made on this thread are [`armed`] or not; the
/// previous setting returns when it is dropped, so a handler that runs Ruby
/// (and so another query) between two passes sets its own.
pub struct Armed(bool);

impl Armed {
    pub fn set(on: bool) -> Armed {
        Armed(ARMED.with(|a| a.replace(on)))
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        /* Runs as a panic in a query unwinds, where a second panic aborts:
         * `try_with`, so a thread being torn down is not one. */
        crate::no_unwind::must_not_unwind::<crate::no_unwind::DropInterruptArmed, _>(|| {
            let _ = ARMED.try_with(|a| a.set(self.0));
        });
    }
}

/// A probe the engine's own tests drive: installed once per process, it answers
/// the calling thread's flag, so a test sets what is "waiting" without Ruby.
#[cfg(test)]
pub(crate) mod test_probe {
    use core::cell::Cell;

    thread_local! {
        static WAITING: Cell<bool> = const { Cell::new(false) };
    }

    pub fn waiting(on: bool) {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| super::install(|| WAITING.with(Cell::get)));
        WAITING.with(|w| w.set(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_nests_and_restores() {
        assert!(!armed());
        let outer = Armed::set(true);
        assert!(armed());
        {
            let _inner = Armed::set(false);
            assert!(!armed());
        }
        assert!(armed());
        drop(outer);
        assert!(!armed());
    }

    #[test]
    fn nothing_is_pending_without_a_probe() {
        assert!(!pending());
    }
}
