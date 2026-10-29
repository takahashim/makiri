//! The hooks an extension's OOM sweep installs, both at once: the raise
//! half (an allocating call of [`crate::raw`] raises `NoMemoryError`) and the
//! allocation half (a checked copy's heap buffer, [`crate::utf8_copy`],
//! fails). One call taking both, so an extension cannot arm one and leave the
//! other half of the sweep failing nothing.

/// What the sweep answers, call by call.
pub struct Injectors {
    /// Whether the next allocating C call raises.
    pub raise: fn() -> bool,
    /// Whether the next fallible heap allocation fails.
    pub alloc: fn() -> bool,
}

/// Install both answers, once, before any method runs (the extension's
/// `Init_`); a second installation is ignored.
pub fn install(i: Injectors) {
    crate::raw::set_raise_injector(i.raise);
    crate::utf8_copy::set_alloc_injector(i.alloc);
}
