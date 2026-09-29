//! The compiled-selector cache for `lexbor::selector_port`'s HTML query, over
//! its OWN process-global parser/arena, built the way the stylesheet reader's
//! is (`css_engine::ParserParts::build()`).
//!
//! # The cache adapts
//!
//! Parsing the selector dominates when the same one is queried repeatedly, so
//! compiled lists are cached in a map keyed by the selector bytes (a Rust map
//! rather than a Ruby Hash, which saves a `VALUE` round-trip on every lookup -
//! it measured ~12% of `matches?`). But holding many distinct lists in the
//! shared arena makes each new parse slower, so a flood of one-off selectors -
//! `getElementById` on unique React `useId` ids, never requeried - turned the
//! cache into a net loss (~22% slower per call). The hit rate is tracked over
//! a window ([`CachePolicy`]); below a floor, the cache is BYPASSED (parse +
//! clean per call, so the arena stays small and the worst case is merely "as
//! fast as no cache"), and caching is periodically re-tested so a workload
//! that starts repeating selectors regains it. (Measured on the OLD
//! `lxb_selectors` engine, whose cache this was; the numbers are the parser's,
//! which both share.)
//!
//! Deliberately NOT `css_parser::ENGINE`: that one is shared with the XML
//! CSS->XPath lowering, whose `Parsed` cleans the WHOLE arena on every drop
//! (its own module doc: "the arena is cleaned on every path out"). A cached
//! entry has to survive many calls without that - so it needs an arena
//! nothing else ever cleans out from under it, which means an engine of its
//! own. `css_parser::list_from_raw` is the one piece of API this module
//! borrows from there: a `Lists` view over a raw pointer THIS module's own
//! cache keeps alive, the same shape `Parsed::groups()` builds over one a
//! `Parsed` keeps alive.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]
// Its one entry point (`with_compiled`) is called from the Ruby glue
// (`glue::html_node::css`) only, so a Ruby-free build (Kani, the fuzz crate,
// `cargo test --features lexbor` without `ruby`) sees this whole file unused -
// same reasoning as `text.rs`'s `VerifiedText` impl.
#![cfg_attr(not(feature = "ruby"), allow(dead_code))]

use core::ptr::NonNull;
use std::collections::HashMap;

use crate::falloc::{try_to_boxed_slice, Reserve};
use crate::gvl::{Gvl, GvlCell, GvlRef};
use crate::lexbor::css_engine::{ParseFail, ParserParts, SelectorParser};
use crate::lexbor::css_parser::{list_from_raw, Lists, ParseError};
use crate::lexbor::selector_port::Scratch;

type SelectorList = crate::lexbor::abi::lxb_css_selector_list_t;

/// Flush the whole cache once it holds this many distinct selectors.
const CACHE_CAP: usize = 256;
/// Re-evaluate the hit rate every N lookups.
const WIN: usize = 1024;
/// Below this hit rate (percent), bypass the cache.
const MIN_HIT_PCT: usize = 15;
/// Re-test caching every N bypass windows.
const RETEST_GAP: usize = 32;

/// Whether the compiled-selector cache is paying for itself.
///
/// Counts lookups over a window of [`WIN`]; at a window's end, a hit rate
/// under [`MIN_HIT_PCT`] switches caching off, and after [`RETEST_GAP`] such
/// windows it is switched back on to be measured again.
struct CachePolicy {
    win: usize,
    win_hits: usize,
    bypass: bool,
    bypass_runs: usize,
}

impl CachePolicy {
    const fn new() -> Self {
        CachePolicy {
            win: 0,
            win_hits: 0,
            bypass: false,
            bypass_runs: 0,
        }
    }

    /// Count one lookup. `true` when this lookup ends a window in which caching
    /// did not pay, so caching is now off and the cached lists must go.
    fn tick(&mut self) -> bool {
        self.win += 1;
        if self.win <= WIN {
            return false;
        }
        let mut switched_off = false;
        if !self.bypass {
            if self.win_hits * 100 < WIN * MIN_HIT_PCT {
                self.bypass = true;
                self.bypass_runs = 0;
                switched_off = true;
            }
        } else {
            self.bypass_runs += 1;
            if self.bypass_runs >= RETEST_GAP {
                self.bypass = false; /* re-test caching over the next window */
            }
        }
        self.win = 1;
        self.win_hits = 0;
        switched_off
    }

    fn hit(&mut self) {
        self.win_hits += 1;
    }

    fn bypassing(&self) -> bool {
        self.bypass
    }
}

/// A compiled selector list in this module's own shared CSS arena.
///
/// The arena owns it, not this: Lexbor never frees one list on its own, and
/// cleaning the arena ([`Cache::flush`], or the bypass path's `clean_all`)
/// invalidates every list in it at once. So this is not `Box`-like and has no
/// `Drop`; what it adds over the raw pointer is that it is non-null, and
/// neither `Copy` nor `Clone`, so a list is only ever LENT - by
/// [`Cache::with_list`] from a `&mut` borrow of the cache, which
/// [`Cache::flush`] needs too, ruling out flushing through the cache under a
/// list in use.
struct CompiledList(NonNull<SelectorList>);

impl CompiledList {
    fn new(p: Result<*mut SelectorList, ParseFail>) -> Result<CompiledList, ParseFail> {
        p.and_then(|l| NonNull::new(l).ok_or(ParseFail::Rejected))
            .map(CompiledList)
    }

    fn as_ptr(&self) -> *mut SelectorList {
        self.0.as_ptr()
    }
}

/// Selector bytes -> the compiled list, which lives in the shared arena.
///
/// The map and the arena are only consistent together, so everything that
/// empties the arena here also empties the map: no entry can outlive the
/// memory it points into.
struct Cache {
    map: Option<HashMap<Box<[u8]>, CompiledList>>,
}

impl Cache {
    const fn new() -> Self {
        Cache { map: None }
    }

    fn map(&mut self) -> &mut HashMap<Box<[u8]>, CompiledList> {
        self.map.get_or_insert_with(HashMap::new)
    }

    /// Run `f` over the compiled list for `selector` - the cached one, which
    /// counts as a hit in `policy`, or one compiled and cached now.
    ///
    /// # Safety
    /// The globals' borrow is live.
    unsafe fn with_list<R>(
        &mut self,
        policy: &mut CachePolicy,
        p: SelectorParser,
        selector: &[u8],
        f: impl FnOnce(&CompiledList) -> R,
    ) -> Result<R, ParseError> {
        if let Some(list) = self.map().get(selector) {
            policy.hit();
            return Ok(f(list));
        }
        // SAFETY: forwarded.
        let list = unsafe { self.compile(p, selector) }?;
        Ok(f(list))
    }

    /// Drop every compiled list: the arena they live in and the map.
    ///
    /// # Safety
    /// The globals' borrow is live, and no cached list is used afterwards.
    unsafe fn flush(&mut self, p: SelectorParser) {
        // SAFETY: forwarded.
        unsafe { p.clean_arena() };
        self.map().clear();
    }

    /// Parse `selector`, cache it, and hand the list back.
    ///
    /// # Safety
    /// The globals' borrow is live.
    unsafe fn compile(
        &mut self,
        p: SelectorParser,
        selector: &[u8],
    ) -> Result<&CompiledList, ParseError> {
        /* Bound the cache BEFORE parsing: when it is full, drop every compiled
         * list at once, so the new list is parsed into the now-empty arena.
         * Flushing after the parse would free the very list just produced. */
        if self.map().len() >= CACHE_CAP {
            // SAFETY: forwarded.
            unsafe { self.flush(p) };
        }

        /* Prepare the owned key and reserve the map before Lexbor allocates the
         * compiled list, so a bookkeeping OOM cannot strand a live list in the
         * shared arena. The key is copied: the borrow points into a Ruby String
         * that may be collected or mutated, while the entry outlives the call.
         * (Folded into `ParseError::Oom`, unlike the old engine's separate
         * `CacheOom` - one "out of memory" is enough detail for the Ruby
         * exception this maps to.) */
        let key = try_to_boxed_slice(selector).ok_or(ParseError::Oom)?;
        if self.map().falloc_reserve(1).is_err() {
            return Err(ParseError::Oom);
        }

        // SAFETY: forwarded.
        let list = CompiledList::new(unsafe { p.parse(selector) });
        /* Return the parser to its CLEAN stage, but do NOT clean the arena -
         * the list just parsed lives there and is about to be cached. */
        // SAFETY: forwarded.
        unsafe { p.clean_parser() };
        let list = match list {
            Ok(list) => list,
            /* The guard could not allocate, so the parser never ran and the
             * arena holds exactly the cached lists it held before: nothing to
             * flush, and not a verdict on the selector. */
            Err(ParseFail::GuardOom) => return Err(ParseError::Oom),
            /* A parse that reached Lexbor and failed drops the cached lists
             * instead of keeping them. This belongs to the same decision as
             * `contains_guard` and goes with it; errors are not a hot path, so
             * the cost is a cold cache. See CLAUDE.md. */
            Err(fail) => {
                // SAFETY: forwarded.
                unsafe { self.flush(p) };
                return Err(parse_error(fail));
            }
        };

        /* Reserved above, so the vacant entry is written without allocating:
         * nothing between the parse and here can fail. `or_insert` rather than
         * an insert so the entry hands back the list it now holds. */
        Ok(self.map().entry(key).or_insert(list))
    }
}

struct Globals {
    engine: Option<SelectorParser>,
    policy: CachePolicy,
    cache: Cache,
    /// The matcher's stacks, kept from one query to the next (its doc).
    scratch: Scratch,
}

/// The one process-global, borrowed once per query by [`Session`] - separate
/// from every other consumer of `css_engine::ParserParts` (module doc).
static G: GvlCell<Globals> = GvlCell::new(Globals {
    engine: None,
    policy: CachePolicy::new(),
    cache: Cache::new(),
    scratch: Scratch::new(),
});

/// Build the shared engine on first use, and hand it back by value. On failure
/// everything is torn down and the globals stay unset, so a later call retries.
fn engine_in(g: &mut Globals) -> Result<SelectorParser, ParseError> {
    if g.engine.is_none() {
        let parts = ParserParts::build().ok_or(ParseError::NotReady)?;
        g.engine = Some(parts.into_parser());
    }
    g.engine.ok_or(ParseError::NotReady)
}

/// Holds the borrow of the globals for one query, and flushes the cache on a
/// PANIC unwind: the engine outlives every call, so a panic mid-parse would
/// leave the shared parser in a non-CLEAN stage and a half-parsed list in the
/// shared arena for every LATER query. Nothing reads a cached list once the
/// query that would have used it has unwound. On the ordinary path it does
/// nothing.
struct Session<'g> {
    g: GvlRef<'g, Globals>,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let Some(parser) = self.g.engine else {
            return;
        };
        // SAFETY: the borrow in `g` is live, and nothing reads a cached list
        // after the query that is unwinding.
        unsafe {
            self.g.cache.flush(parser);
            parser.clean_parser();
        }
    }
}

/// Parse `selector` with this module's own shared engine (or serve it from
/// the cache), hand the compiled selector list to `f` as a [`Lists`] view -
/// with the matcher's kept [`Scratch`] - then leave the engine ready for the
/// next call.
///
/// Three ways: a window that ended with caching off flushes the cache;
/// bypassing parses and cleans per call; otherwise the list comes from the
/// cache, compiled into it on a miss.
pub(crate) fn with_compiled<R>(
    gvl: &Gvl,
    selector: &[u8],
    f: impl FnOnce(Lists<'_>, &mut Scratch) -> R,
) -> Result<R, ParseError> {
    let mut g = G.borrow(gvl).map_err(|_| ParseError::Busy)?;
    let e = engine_in(&mut g)?;
    let mut session = Session { g };
    let g = &mut *session.g;

    if g.policy.tick() {
        // SAFETY: the session's borrow is live.
        unsafe { g.cache.flush(e) };
    }

    if g.policy.bypassing() {
        /* Parse + clean per call - the behaviour before the cache existed - so
         * the arena stays small and a one-off-selector flood is no slower than
         * having no cache at all. */
        // SAFETY: the session's borrow is live.
        let parsed = CompiledList::new(unsafe { e.parse(selector) });
        let result = match parsed {
            // SAFETY: `list` points into the arena `e` owns, kept alive for
            // this call by the session's live borrow; `list_from_raw`'s
            // contract.
            Ok(list) => Ok(f(unsafe { list_from_raw(list.as_ptr()) }, &mut g.scratch)),
            Err(fail) => Err(parse_error(fail)),
        };
        // SAFETY: `list` (if any) is out of scope before the arena it lives
        // in is cleaned; the session's borrow is live.
        unsafe { e.clean_all() };
        return result;
    }

    /* The cached list and its arena stay - only the map/policy bookkeeping
     * changes. */
    // SAFETY: the session's borrow is live.
    unsafe {
        g.cache.with_list(&mut g.policy, e, selector, |list| {
            f(list_from_raw(list.as_ptr()), &mut g.scratch)
        })
    }
}

/// The error a failed parse is reported as: an allocation failure is not a
/// verdict on the selector.
fn parse_error(fail: ParseFail) -> ParseError {
    match fail {
        ParseFail::Rejected => ParseError::Syntax,
        ParseFail::GuardOom | ParseFail::ParserOom => ParseError::Oom,
    }
}
