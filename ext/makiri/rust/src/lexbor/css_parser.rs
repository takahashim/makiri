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

#![allow(unsafe_code)]

//! # Unsafe boundary
//!
//! Lexbor owns this parser and exposes it as three raw pointers. This module is
//! the only CSS module allowed to retain those pointers or mutate the
//! process-global parser state. CSS compilation holds Ruby's GVL, which is the
//! serialisation mechanism documented by [`GvlEngine`].
//!
//! It is also the only reader of what a parse builds: the lowering walks the
//! selectors through [`List`] and [`Selector`], borrowed from [`Parsed`], so
//! no pointer into the arena outlives the parse and every union read is keyed
//! by the tag Lexbor stored beside it.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use crate::lexbor_abi as lxb;
use crate::lexbor_abi::{
    lxb_css_memory_clean, lxb_css_memory_create, lxb_css_memory_destroy, lxb_css_memory_init,
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    lxb_css_parser_memory_set_noi, lxb_css_parser_selectors_set_noi, lxb_css_parser_status_noi,
    lxb_css_selectors_create, lxb_css_selectors_destroy, lxb_css_selectors_init,
    lxb_css_selectors_parse, CssMemory, CssParser, CssSelectors,
};
use crate::text::VerifiedText;
use core::ffi::c_long;

/// `lxb_css_selector_list_t`, generated.
type SelectorList = lxb::lxb_css_selector_list_t;
type RawSelector = lxb::lxb_css_selector_t;

use crate::lexbor_abi::consts::STATUS_OK as LXB_STATUS_OK;

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
        // checked before initialization or destruction. One block for the
        // three, because the sentence above is the contract for all of them.
        let (mem, parser, sel) = unsafe {
            (
                lxb_css_memory_create(),
                lxb_css_parser_create(),
                lxb_css_selectors_create(),
            )
        };
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
    first: *mut SelectorList,
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
/// The process-global parser is used under the GVL (CSS never releases it),
/// and `selector` is a live verified slice, so this is safe to call as-is.
pub fn parse(selector: VerifiedText) -> Result<Parsed, ParseError> {
    // SAFETY: caller contract holds the GVL for the complete `Parsed` lifetime.
    let e = unsafe { ENGINE.ready() }.ok_or(ParseError::NotReady)?;

    // SAFETY: selector is a live verified slice and `e.parser` is initialized.
    let list = unsafe {
        lxb_css_selectors_parse(e.parser, selector.as_ptr() as *const u8, selector.len())
    };
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

/* ------------------------------------------------------------------ *
 * the parse, read through typed views                                *
 * ------------------------------------------------------------------ */

/* The selector kinds, combinators, match operators and pseudo ids, generated. */
pub(crate) mod k {
    use crate::lexbor_abi as l;
    pub const ANY: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ANY;
    pub const ELEMENT: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ELEMENT;
    pub const ID: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ID;
    pub const CLASS: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_CLASS;
    pub const ATTRIBUTE: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ATTRIBUTE;
    pub const PSEUDO_CLASS: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS;
    pub const PSEUDO_CLASS_FUNCTION: u32 =
        l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS_FUNCTION;
    pub const PSEUDO_ELEMENT: u32 = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT;
    pub const PSEUDO_ELEMENT_FUNCTION: u32 =
        l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT_FUNCTION;
}

pub(crate) mod comb {
    use crate::lexbor_abi as l;
    pub const CLOSE: u32 = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_CLOSE;
    pub const CHILD: u32 = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_CHILD;
    pub const SIBLING: u32 = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_SIBLING;
    pub const FOLLOWING: u32 =
        l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_FOLLOWING;
}

pub(crate) mod m {
    use crate::lexbor_abi as l;
    pub const EQUAL: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_EQUAL;
    pub const INCLUDE: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_INCLUDE;
    pub const PREFIX: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_PREFIX;
    pub const SUBSTRING: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_SUBSTRING;
    pub const SUFFIX: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_SUFFIX;
    pub const DASH: u32 = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_DASH;
    pub const MOD_I: u32 = l::lxb_css_selector_modifier_t_LXB_CSS_SELECTOR_MODIFIER_I;
    pub const MOD_S: u32 = l::lxb_css_selector_modifier_t_LXB_CSS_SELECTOR_MODIFIER_S;
}

pub(crate) mod pc {
    use crate::lexbor_abi as l;
    pub const FIRST_CHILD: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_CHILD;
    pub const LAST_CHILD: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_CHILD;
    pub const ONLY_CHILD: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_CHILD;
    pub const EMPTY: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_EMPTY;
    pub const ROOT: u32 = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ROOT;
    pub const FIRST_OF_TYPE: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_OF_TYPE;
    pub const LAST_OF_TYPE: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_OF_TYPE;
    pub const ONLY_OF_TYPE: u32 =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_OF_TYPE;
}

pub(crate) mod pf {
    use crate::lexbor_abi as l;
    type T = l::lxb_css_selector_pseudo_class_function_id_t;
    pub const NTH_CHILD: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_CHILD;
    pub const NTH_LAST_CHILD: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_CHILD;
    pub const NTH_OF_TYPE: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_OF_TYPE;
    pub const NTH_LAST_OF_TYPE: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_OF_TYPE;
    pub const NOT: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NOT;
    pub const IS: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_IS;
    pub const WHERE: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_WHERE;
    pub const HAS: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_HAS;
    pub const LEXBOR_CONTAINS: T =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_LEXBOR_CONTAINS;
}

/// A reference into the arena a parse built.
///
/// # Safety
/// `p` must be null or point into the arena of a [`Parsed`] that lives for
/// `'p`.
unsafe fn arena<'p, T>(p: *const T) -> Option<&'p T> {
    // SAFETY: forwarded.
    unsafe { p.as_ref() }
}

/// A `lexbor_str_t` as a slice, or `None` when its data pointer is NULL.
///
/// The NULL-vs-empty distinction is load-bearing: for a namespace, NULL means
/// "no pipe was written" and empty means "an explicit no-namespace".
///
/// # Safety
/// `s` must be a string Lexbor built in a parse's arena.
unsafe fn str_opt(s: &lxb::lexbor_str_t) -> Option<&[u8]> {
    if s.data.is_null() {
        None
    } else {
        // SAFETY: Lexbor keeps `length` bytes at `data`, in the same arena.
        Some(unsafe { core::slice::from_raw_parts(s.data, s.length) })
    }
}

impl Parsed {
    /// The selector's comma groups, in order.
    pub fn groups(&self) -> Lists<'_> {
        // SAFETY: `first` is null or the list Lexbor built, which the arena
        // keeps until `self` drops.
        Lists(unsafe { arena(self.first) }.map(List))
    }
}

/// A selector list: a comma group of the query, or the argument of a
/// functional pseudo-class.
#[derive(Clone, Copy)]
pub struct List<'p>(&'p SelectorList);

impl<'p> List<'p> {
    /// The first simple selector of the list's chain.
    pub fn first(self) -> Option<Selector<'p>> {
        // SAFETY: a list's links point into its own arena.
        unsafe { arena(self.0.first) }.map(Selector)
    }
}

/// Selector lists followed through their `next` links.
#[derive(Clone, Copy)]
pub struct Lists<'p>(Option<List<'p>>);

impl<'p> Iterator for Lists<'p> {
    type Item = List<'p>;

    fn next(&mut self) -> Option<List<'p>> {
        let cur = self.0?;
        // SAFETY: as in `List::first`.
        self.0 = unsafe { arena(cur.0.next) }.map(List);
        Some(cur)
    }
}

/// One simple selector of a chain.
#[derive(Clone, Copy)]
pub struct Selector<'p>(&'p RawSelector);

/// Identity: two views of the same selector.
impl PartialEq for Selector<'_> {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0, other.0)
    }
}

/// An attribute selector's operator, case modifier and value.
pub struct Attribute<'p> {
    pub match_: u32,
    pub modifier: u32,
    /// None for `[name]`, an existence test.
    pub value: Option<&'p [u8]>,
}

/// `:nth-*(an+b [of S])`.
pub struct Nth {
    pub a: c_long,
    pub b: c_long,
    /// Whether an `of S` clause was written.
    pub of: bool,
}

/// `:lexbor-contains(needle [i])`.
pub struct Contains<'p> {
    pub needle: &'p [u8],
    pub insensitive: bool,
}

/// A functional pseudo-class's argument, as what its id says Lexbor stored.
pub enum FunctionArg<'p> {
    /// The `:nth-*` family, or None when Lexbor stored nothing.
    Nth(Option<Nth>),
    /// `:not`, `:is`, `:where` and `:has`.
    Selectors(Lists<'p>),
    /// `:lexbor-contains`, or None when Lexbor stored nothing.
    Contains(Option<Contains<'p>>),
    /// Anything else, or not a functional pseudo-class.
    Other,
}

impl<'p> Selector<'p> {
    pub fn kind(self) -> u32 {
        self.0.type_
    }

    pub fn combinator(self) -> u32 {
        self.0.combinator
    }

    /// The name, empty when there is none.
    pub fn name(self) -> &'p [u8] {
        // SAFETY: a selector's strings live in its arena.
        unsafe { str_opt(&self.0.name) }.unwrap_or(&[])
    }

    /// The namespace as written: None with no pipe, empty for `|name`.
    pub fn ns(self) -> Option<&'p [u8]> {
        // SAFETY: as in `name`.
        unsafe { str_opt(&self.0.ns) }
    }

    /// The next simple selector of the chain.
    pub fn next(self) -> Option<Selector<'p>> {
        // SAFETY: a selector's links point into its own arena.
        unsafe { arena(self.0.next) }.map(Selector)
    }

    /// An attribute selector's parts; None for any other kind.
    pub fn attribute(self) -> Option<Attribute<'p>> {
        if self.kind() != k::ATTRIBUTE {
            return None;
        }
        // SAFETY: Lexbor fills `u.attribute` for an attribute selector.
        let at = unsafe { &self.0.u.attribute };
        Some(Attribute {
            match_: at.match_,
            modifier: at.modifier,
            // SAFETY: as in `name`.
            value: unsafe { str_opt(&at.value) },
        })
    }

    /// A pseudo-class's id, plain or functional; None for any other kind.
    pub fn pseudo_id(self) -> Option<u32> {
        if !matches!(self.kind(), k::PSEUDO_CLASS | k::PSEUDO_CLASS_FUNCTION) {
            return None;
        }
        // SAFETY: Lexbor fills `u.pseudo` for a pseudo-class.
        Some(unsafe { self.0.u.pseudo.type_ })
    }

    /// A functional pseudo-class's argument.
    pub fn function_arg(self) -> FunctionArg<'p> {
        if self.kind() != k::PSEUDO_CLASS_FUNCTION {
            return FunctionArg::Other;
        }
        // SAFETY: as in `pseudo_id`.
        let pseudo = unsafe { self.0.u.pseudo };
        let data = pseudo.data as *const core::ffi::c_void;
        match pseudo.type_ {
            pf::NTH_CHILD | pf::NTH_LAST_CHILD | pf::NTH_OF_TYPE | pf::NTH_LAST_OF_TYPE => {
                // SAFETY: for the `:nth-*` family Lexbor stores an
                // `lxb_css_selector_anb_of_t`, in the arena.
                let anb = unsafe { arena(data as *const lxb::lxb_css_selector_anb_of_t) };
                FunctionArg::Nth(anb.map(|n| Nth {
                    a: n.anb.a,
                    b: n.anb.b,
                    of: !n.of.is_null(),
                }))
            }
            pf::NOT | pf::IS | pf::WHERE | pf::HAS => {
                // SAFETY: for these Lexbor stores a selector list, in the arena.
                FunctionArg::Selectors(Lists(
                    unsafe { arena(data as *const SelectorList) }.map(List),
                ))
            }
            pf::LEXBOR_CONTAINS => {
                // SAFETY: Lexbor stores an `lxb_css_selector_contains_t`, in the
                // arena.
                let c = unsafe { arena(data as *const lxb::lxb_css_selector_contains_t) };
                FunctionArg::Contains(c.map(|c| Contains {
                    // SAFETY: as in `name`.
                    needle: unsafe { str_opt(&c.str_) }.unwrap_or(&[]),
                    insensitive: c.insensitive,
                }))
            }
            _ => FunctionArg::Other,
        }
    }
}

/// Release the engine. Not called: the C had no teardown either, because the
/// parser lives for the process. Present so the allocation is visibly owned
/// rather than merely leaked.
#[allow(dead_code)]
pub unsafe fn shutdown() {
    // SAFETY: forwarded contract; teardown is test-only.
    unsafe { ENGINE.shutdown() };
}
