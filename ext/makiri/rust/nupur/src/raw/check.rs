//! Conversions and the checks a C function would make and does not.

use super::*;

/// `rb_sym2str`: raises `NoMemoryError` when a static Symbol's name String is
/// made for the first time. Runs no Ruby code.
#[inline]
pub fn sym2str(sym: Symbol) -> Result<RString, Error> {
    let v = sym.as_raw();
    let s = call(|| unsafe {
        injected_raise();
        nupur_sys::rb_sym2str(v)
    })?;
    typed(s, RString::from_value, "a C call made no String")
}

/// `rb_check_frozen` returning its `FrozenError` rather than raising it.
///
/// The error is Ruby's own - the message naming the receiver and `#receiver`
/// set - because the check still runs, protected. An unfrozen value, every
/// mutator's normal case, makes no call.
#[inline]
pub fn check_frozen(v: Value) -> Result<(), Error> {
    if !crate::rb::ReprValue::is_frozen(v) {
        return Ok(());
    }
    let v = v.as_raw();
    call(|| unsafe { nupur_sys::rb_check_frozen(v) })
}

/// `rb_range_beg_len` with `err = 0` over a collection of `count` elements:
/// the start and the length it selects, `None` for a start outside it, and
/// `RangeError` for a bound too large for a `long`. A Range whose ends are
/// not Integers or nil is a `TypeError`, checked here: the C would convert
/// such an end with its `to_int`.
#[inline]
pub fn range_beg_len(r: Range, count: usize) -> Result<Option<(usize, usize)>, Error> {
    /* Checked here, not left to the caller: an end of any other kind would
     * have its `to_int` run by the C, the caller's Ruby. */
    let (beg, end, _) = range_values(r)?;
    if [beg, end]
        .iter()
        .any(|&e| !crate::rb::ReprValue::is_nil(e) && Integer::from_value(e).is_none())
    {
        return Err(crate::rb::error::type_error(
            "a Range whose ends are Integers (or nil) was expected",
        ));
    }
    let count = count
        .try_into()
        .map_err(|_| crate::rb::error::arg_error("a collection too large for a long"))?;
    let (mut beg, mut len) = (0, 0);
    let (b, l) = (core::ptr::addr_of_mut!(beg), core::ptr::addr_of_mut!(len));
    let r = r.as_raw();
    let answer = call(|| unsafe { nupur_sys::rb_range_beg_len(r, b, l, count, 0) })?;
    if answer != nupur_sys::Qtrue as VALUE {
        return Ok(None);
    }
    /* Within `0..=count` both, when the C answers true. */
    match (usize::try_from(beg), usize::try_from(len)) {
        (Ok(beg), Ok(len)) => Ok(Some((beg, len))),
        _ => Err(crate::rb::error::internal_error(
            "rb_range_beg_len answered a negative bound",
        )),
    }
}

/// `rb_thread_check_ints`: raises a pending interrupt (`Interrupt`,
/// `Thread#raise`'s exception, a `Timeout`), or ends the thread.
#[inline]
pub fn thread_check_ints(_: &Ruby) -> Result<(), Error> {
    call(|| unsafe { nupur_sys::rb_thread_check_ints() })
}
