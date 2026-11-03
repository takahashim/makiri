//! The places in the Ruby layer that must not unwind, and the check that they
//! cannot - the layer's own copy of the mechanism. It lives here, under
//! nupur, because the protect thunk does ([`crate::protect`]), which is the
//! one site of this crate's; Nupur declares its own (the GC callbacks, the
//! trampolines, its guards' `Drop`s) with [`no_unwind_sites!`](crate::no_unwind_sites) in
//! `nupur::no_unwind`. An extension keeps its own for its types' `Drop`s and
//! the engine's: the engine is built without Ruby, where this module does not
//! exist.
//!
//! An unwind out of an `extern "C"` function aborts the process (Rust stops it
//! at the boundary, since C frames have no unwind tables), and so does a panic
//! in a `Drop` that runs while another panic unwinds - or in a GC free
//! callback, which is both. Each such body is listed here as a [`Site`], and
//! runs inside [`must_not_unwind`].
//!
//! In a normal build that costs nothing: a site's guard is a zero-sized value
//! with no `Drop`. Built with `--cfg nupur_no_unwind` (`rake no_unwind`), each
//! site's guard gets a `Drop` that calls a symbol nobody defines,
//! `nupur_unwinds_<site>` - and the only path that runs it is an unwind out
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

/// Declare sites: each a [`Site`] whose guard, in the check build
/// (`--cfg nupur_no_unwind`), calls the undefined symbol named beside it.
/// Exported so that Nupur declares its own sites where they are, and this
/// crate only the ones it holds; the cfg is the crate's that expands it,
/// which the check build sets for both.
#[macro_export]
macro_rules! no_unwind_sites {
    ($($(#[$doc:meta])* $name:ident => $sym:literal,)*) => {$(
        $(#[$doc])*
        pub struct $name(());
        impl $crate::no_unwind::Site for $name {
            #[inline(always)]
            fn arm() -> $name {
                $name(())
            }
        }
        #[cfg(nupur_no_unwind)]
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

no_unwind_sites! {
    /// [`crate::protect::call`]'s `rb_protect` thunk.
    ProtectThunk => "nupur_unwinds_protect_thunk",
    /// The check's own control (`self_test`): a body that CAN unwind, so a
    /// check build that still works leaves exactly this symbol undefined.
    SelfTest => "nupur_unwinds_self_test",
}

/// The control for `rake no_unwind`, called from the extension's `Init_` in the check
/// build only: were the guard ever optimized away whole, every site would
/// pass, and this one would too - which the task refuses.
#[cfg(nupur_no_unwind)]
#[allow(clippy::panic, reason = "the check's control must be able to unwind")]
pub fn self_test() {
    must_not_unwind::<SelfTest, _>(|| {
        if core::hint::black_box(false) {
            panic!("no_unwind self-test");
        }
    })
}
