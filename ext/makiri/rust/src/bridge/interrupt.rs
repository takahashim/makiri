//! The pending-interrupt check (`nupur::interrupt::interrupt_probe`), installed
//! into `crate::interrupt` at `Init_makiri` so the engine's budgets can ask it
//! without knowing Ruby exists. See `crate::interrupt` for why an evaluation
//! stops to deliver rather than checking in place.

#![forbid(unsafe_code)]

/// The probe "has the current thread an interrupt waiting?", for
/// `crate::interrupt::install`.
pub(crate) fn probe(ruby: &crate::bridge::rb::Ruby) -> fn() -> bool {
    crate::nupur::interrupt::interrupt_probe(ruby)
}

/// What one pass of a query came to.
pub enum Pass<T> {
    /// The query ran to its end: this is its answer.
    Done(T),
    /// The query stopped for an interrupt and holds nothing now.
    Interrupted,
}

/// Run a query so that an interrupt reaches the thread: `pass` runs it with
/// [`crate::interrupt::armed`] set, and returns [`Pass::Interrupted`] once it
/// has stopped and let go of every guard and borrow it took. The interrupt is
/// delivered here, with none held (`rb_thread_check_ints`: a `Timeout` or
/// `Thread#raise` comes back as `Err`; a time-slice switch or a signal handler
/// runs and returns), and the query runs again from the start - which only
/// reads, so the answer is the same, or that of the document as another thread
/// left it. At most [`PASSES`](crate::interrupt::PASSES) passes: the last is not
/// armed (a time-slice flag is raised again and again, and cannot be told from a
/// `Timeout`), so a query always finishes.
pub fn interruptible<T>(
    ruby: &crate::bridge::rb::Ruby,
    mut pass: impl FnMut() -> Result<Pass<T>, crate::bridge::rb::Error>,
) -> Result<T, crate::bridge::rb::Error> {
    for n in 1..=crate::interrupt::PASSES {
        let outcome = {
            let _armed = crate::interrupt::Armed::set(n < crate::interrupt::PASSES);
            pass()?
        };
        match outcome {
            Pass::Done(answer) => return Ok(answer),
            Pass::Interrupted => crate::bridge::raw::thread_check_ints(ruby)?,
        }
    }
    Err(crate::bridge::ruby::internal_error(
        "a query that was not asked to stop was stopped",
    ))
}
