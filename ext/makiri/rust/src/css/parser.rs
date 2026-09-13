//! The process-global Lexbor CSS parser - selector parsing only, NOT the
//! matcher.
//!
//! CSS compilation runs under the GVL (the glue holds it throughout), so a
//! single global is safe with no locking, the same argument the HTML CSS engine
//! makes. Created lazily; a creation failure is reported to the caller rather
//! than retried, and leaves the globals untouched so a later call tries again.
//!
//! # The arena is cleaned on every path out
//!
//! The parsed selector list lives in the parser's memory arena, which is reset
//! between calls. The C reset it at each of its three return points; here that
//! is a `Drop`, so a path added later cannot forget - and the lowering borrows
//! the list, so the reset must not happen before the lowering is done.

//! # Unsafe boundary
//!
//! Lexbor owns this parser and exposes it as three raw pointers. This module is
//! the only CSS module allowed to retain those pointers or mutate the
//! process-global parser state. CSS compilation holds Ruby's GVL, which is the
//! serialisation mechanism documented by [`GvlEngine`].

#![deny(unsafe_op_in_unsafe_fn)]

use crate::lexbor_abi::{
    lxb_css_memory_clean, lxb_css_memory_create, lxb_css_memory_destroy, lxb_css_memory_init,
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    lxb_css_parser_memory_set_noi, lxb_css_parser_selectors_set_noi, lxb_css_parser_status_noi,
    lxb_css_selectors_create, lxb_css_selectors_destroy, lxb_css_selectors_init,
    lxb_css_selectors_parse, CssMemory, CssParser, CssSelectors,
};
use crate::xpath_abi::VerifiedText;

/// `lxb_css_selector_list_t`, generated.
pub type SelectorList = crate::lexbor_abi::lxb_css_selector_list_t;

const LXB_STATUS_OK: u32 = crate::lexbor_abi::lexbor_status_t_LXB_STATUS_OK;

/// Why a parse did not produce a selector list.
pub enum ParseError {
    /// The parser could not be created.
    NotReady,
    /// The selector is malformed.
    Syntax,
}

#[derive(Clone, Copy)]
struct Engine {
    mem: *mut CssMemory,
    parser: *mut CssParser,
    sel: *mut CssSelectors,
}

/// Process-global CSS parser state, accessed only while Ruby's GVL is held.
///
/// `UnsafeCell` confines the required interior mutability to this one type.
/// The GVL serialises every caller, so no mutex is needed and no mutable
/// reference can escape to the lowering layer.
struct GvlEngine(core::cell::UnsafeCell<Option<Engine>>);

// SAFETY: all access is from GVL-held Ruby glue; see `GvlEngine`.
unsafe impl Sync for GvlEngine {}

impl GvlEngine {
    const fn new() -> Self {
        Self(core::cell::UnsafeCell::new(None))
    }

    /// # Safety
    /// The caller holds the GVL until the returned engine has been cleaned.
    unsafe fn ready(&self) -> Option<Engine> {
        // SAFETY: the GVL makes this global access exclusive.
        let slot = unsafe { &mut *self.0.get() };
        if let Some(e) = *slot {
            return Some(e);
        }

        // SAFETY: constructors do not borrow Rust memory; every pointer is
        // checked before initialization or destruction.
        let mem = unsafe { lxb_css_memory_create() };
        let parser = unsafe { lxb_css_parser_create() };
        let sel = unsafe { lxb_css_selectors_create() };
        let ok = !mem.is_null()
            && !parser.is_null()
            && !sel.is_null()
            // SAFETY: all pointers above were checked non-null.
            && unsafe { lxb_css_memory_init(mem, 128) } == LXB_STATUS_OK
            // SAFETY: all pointers above were checked non-null.
            && unsafe { lxb_css_parser_init(parser, core::ptr::null_mut()) } == LXB_STATUS_OK
            // SAFETY: all pointers above were checked non-null.
            && unsafe { lxb_css_selectors_init(sel) } == LXB_STATUS_OK;
        if !ok {
            if !sel.is_null() {
                // SAFETY: created by this call and not yet destroyed.
                unsafe { lxb_css_selectors_destroy(sel, true) };
            }
            if !parser.is_null() {
                // SAFETY: created by this call and not yet destroyed.
                unsafe { lxb_css_parser_destroy(parser, true) };
            }
            if !mem.is_null() {
                // SAFETY: created by this call and not yet destroyed.
                unsafe { lxb_css_memory_destroy(mem, true) };
            }
            return None;
        }

        // SAFETY: initialized live Lexbor objects; the GVL prevents a race.
        unsafe { lxb_css_parser_memory_set_noi(parser, mem) };
        // SAFETY: initialized live Lexbor objects; the GVL prevents a race.
        unsafe { lxb_css_parser_selectors_set_noi(parser, sel) };
        let engine = Engine { mem, parser, sel };
        *slot = Some(engine);
        Some(engine)
    }

    /// # Safety
    /// The caller holds the GVL and no selector from `engine` is used later.
    unsafe fn clean(&self, engine: Engine) {
        // SAFETY: `engine` is initialized by `ready` and use is GVL-serial.
        unsafe { lxb_css_memory_clean(engine.mem) };
        // SAFETY: same initialized parser as above.
        unsafe { lxb_css_parser_clean(engine.parser) };
    }

    /// # Safety
    /// The caller holds the GVL and no `Parsed` value remains alive.
    #[allow(dead_code)]
    unsafe fn shutdown(&self) {
        // SAFETY: the GVL excludes all concurrent accesses.
        let slot = unsafe { &mut *self.0.get() };
        if let Some(e) = slot.take() {
            // SAFETY: all pointers belong to this initialized engine.
            unsafe { lxb_css_selectors_destroy(e.sel, true) };
            // SAFETY: all pointers belong to this initialized engine.
            unsafe { lxb_css_parser_destroy(e.parser, true) };
            // SAFETY: all pointers belong to this initialized engine.
            unsafe { lxb_css_memory_destroy(e.mem, true) };
        }
    }
}

static ENGINE: GvlEngine = GvlEngine::new();

/// A parsed selector list, borrowed from the engine's arena.
///
/// Dropping it cleans the arena and returns the parser to its CLEAN stage, which
/// is what makes the next call safe. The list must not be read afterwards - the
/// lifetime says so.
pub struct Parsed {
    pub first: *mut SelectorList,
    engine: Engine,
}

impl Drop for Parsed {
    fn drop(&mut self) {
        // SAFETY: `Parsed` is constructed under the GVL and owns the interval
        // in which `first` may be read.
        unsafe {
            ENGINE.clean(self.engine);
        }
    }
}

/// Parse `selector` into the engine's arena.
///
/// # Safety
/// Under the GVL, with `selector` a live verified slice.
pub unsafe fn parse(selector: VerifiedText) -> Result<Parsed, ParseError> {
    // SAFETY: caller contract holds the GVL for the complete `Parsed` lifetime.
    let e = unsafe { ENGINE.ready() }.ok_or(ParseError::NotReady)?;

    // SAFETY: selector is a live verified slice and `e.parser` is initialized.
    let list =
        unsafe { lxb_css_selectors_parse(e.parser, selector.ptr as *const u8, selector.len) };
    /* Both conditions matter: Lexbor can hand back a list AND a non-OK status
     * for a partially-recovered parse, and a recovered selector is not the one
     * the caller wrote. */
    if list.is_null() || unsafe { lxb_css_parser_status_noi(e.parser) } != LXB_STATUS_OK {
        drop(Parsed {
            first: core::ptr::null_mut(),
            engine: e,
        }); /* clean the arena */
        return Err(ParseError::Syntax);
    }
    Ok(Parsed {
        first: list,
        engine: e,
    })
}

/// Release the engine. Not called: the C had no teardown either, because the
/// parser lives for the process. Present so the allocation is visibly owned
/// rather than merely leaked.
#[allow(dead_code)]
pub unsafe fn shutdown() {
    // SAFETY: forwarded contract; teardown is test-only.
    unsafe { ENGINE.shutdown() };
}
