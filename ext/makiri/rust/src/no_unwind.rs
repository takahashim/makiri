//! The places that must not unwind, and the check that they cannot.
//!
//! An unwind out of an `extern "C"` function aborts the process (Rust stops it
//! at the boundary, since C frames have no unwind tables), and so does a panic
//! in a `Drop` that runs while another panic unwinds - or in a GC free
//! callback, which is both. Each such body is listed here as a [`Site`], and
//! runs inside [`must_not_unwind`].
//!
//! In a normal build that costs nothing: a site's guard is a zero-sized value
//! with no `Drop`. Built with `--cfg makiri_no_unwind` (`rake no_unwind`), each
//! site's guard gets a `Drop` that calls a symbol nobody defines,
//! `makiri_unwinds_<site>` - and the only path that runs it is an unwind out
//! of the body, since a normal return forgets it. The optimizer removes that
//! path exactly when it can prove the body never unwinds, so the symbol is
//! left undefined in the release build precisely for a site that CAN unwind:
//! the check is the compiler's own proof, not a reading of the code. (The
//! `no-panic` crate's technique, without the dependency; macOS links an
//! undefined symbol without complaint, so the task reads them with `nm -u`.)

#![allow(unsafe_code)]

/// A body that must not unwind: its guard is armed before the body runs and
/// disarmed after, so only an unwind drops it.
pub trait Site: Sized {
    fn arm() -> Self;
    #[inline(always)]
    fn disarm(self) {
        core::mem::forget(self);
    }
}

/// Run `body` as `S`, a site that must not unwind.
#[inline(always)]
pub fn must_not_unwind<S: Site, R>(body: impl FnOnce() -> R) -> R {
    let armed = S::arm();
    let out = body();
    armed.disarm();
    out
}

macro_rules! sites {
    ($($(#[$doc:meta])* $name:ident => $sym:literal,)*) => {$(
        $(#[$doc])*
        pub struct $name(());
        impl Site for $name {
            #[inline(always)]
            fn arm() -> $name {
                $name(())
            }
        }
        #[cfg(makiri_no_unwind)]
        impl Drop for $name {
            #[inline(always)]
            fn drop(&mut self) {
                unsafe extern "C" {
                    #[link_name = $sym]
                    fn unwinds();
                }
                // SAFETY: never defined, so never called: a build that keeps
                // this call fails the check rather than running it.
                unsafe { unwinds() }
            }
        }
    )*};
}

sites! {
    /// `xpath::ctx::Running`'s `Drop`, run as a panic in an evaluate unwinds.
    DropXPathRunning => "makiri_unwinds_drop_xpath_running",
    /// The check's own control ([`self_test`]): a body that CAN unwind, so a
    /// check build that still works leaves exactly this symbol undefined.
    SelfTest => "makiri_unwinds_self_test",
}

/// The control for `rake no_unwind`, called from `Init_makiri` in the check
/// build only: were the guard ever optimized away whole, every site would
/// pass, and this one would too - which the task refuses.
#[cfg(makiri_no_unwind)]
#[allow(clippy::panic, reason = "the check's control must be able to unwind")]
pub fn self_test() {
    must_not_unwind::<SelfTest, _>(|| {
        if core::hint::black_box(false) {
            panic!("no_unwind self-test");
        }
    })
}
