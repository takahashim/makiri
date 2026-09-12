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

struct Engine {
    mem: *mut CssMemory,
    parser: *mut CssParser,
    sel: *mut CssSelectors,
}

static mut ENGINE: Option<Engine> = None;

/// Build the engine once. `None` if any piece could not be created, with
/// everything released - so the next call retries from nothing rather than
/// inheriting a half-built object.
unsafe fn ready() -> Option<&'static Engine> {
    #[allow(static_mut_refs)]
    if let Some(e) = ENGINE.as_ref() {
        return Some(e);
    }

    let mem = lxb_css_memory_create();
    let parser = lxb_css_parser_create();
    let sel = lxb_css_selectors_create();

    let ok = !mem.is_null()
        && !parser.is_null()
        && !sel.is_null()
        && lxb_css_memory_init(mem, 128) == LXB_STATUS_OK
        && lxb_css_parser_init(parser, core::ptr::null_mut())
            == LXB_STATUS_OK
        && lxb_css_selectors_init(sel) == LXB_STATUS_OK;
    if !ok {
        if !sel.is_null() {
            lxb_css_selectors_destroy(sel, true);
        }
        if !parser.is_null() {
            lxb_css_parser_destroy(parser, true);
        }
        if !mem.is_null() {
            lxb_css_memory_destroy(mem, true);
        }
        return None;
    }

    lxb_css_parser_memory_set_noi(parser, mem);
    lxb_css_parser_selectors_set_noi(parser, sel);
    ENGINE = Some(Engine { mem, parser, sel });
    #[allow(static_mut_refs)]
    ENGINE.as_ref()
}

/// A parsed selector list, borrowed from the engine's arena.
///
/// Dropping it cleans the arena and returns the parser to its CLEAN stage, which
/// is what makes the next call safe. The list must not be read afterwards - the
/// lifetime says so.
pub struct Parsed {
    pub first: *mut SelectorList,
}

impl Drop for Parsed {
    fn drop(&mut self) {
        unsafe {
            #[allow(static_mut_refs)]
            if let Some(e) = ENGINE.as_ref() {
                lxb_css_memory_clean(e.mem);
                lxb_css_parser_clean(e.parser);
            }
        }
    }
}

/// Parse `selector` into the engine's arena.
///
/// # Safety
/// Under the GVL, with `selector` a live verified slice.
pub unsafe fn parse(selector: VerifiedText) -> Result<Parsed, ParseError> {
    let e = ready().ok_or(ParseError::NotReady)?;

    let list = lxb_css_selectors_parse(e.parser, selector.ptr as *const u8, selector.len);
    /* Both conditions matter: Lexbor can hand back a list AND a non-OK status
     * for a partially-recovered parse, and a recovered selector is not the one
     * the caller wrote. */
    if list.is_null() || lxb_css_parser_status_noi(e.parser) != LXB_STATUS_OK
    {
        drop(Parsed { first: core::ptr::null_mut() }); /* clean the arena */
        return Err(ParseError::Syntax);
    }
    Ok(Parsed { first: list })
}

/// Release the engine. Not called: the C had no teardown either, because the
/// parser lives for the process. Present so the allocation is visibly owned
/// rather than merely leaked.
#[allow(dead_code, static_mut_refs)]
pub unsafe fn shutdown() {
    if let Some(e) = ENGINE.take() {
        lxb_css_selectors_destroy(e.sel, true);
        lxb_css_parser_destroy(e.parser, true);
        lxb_css_memory_destroy(e.mem, true);
    }
}
