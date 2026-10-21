//! The compiled-selector cache for `css_match`'s HTML query: a
//! selector string is parsed once by the ported parser (`crate::selectors`)
//! and its AST kept for the next query that uses the same string.
//!
//! # The cache adapts
//!
//! Compiled selectors are kept in a map keyed by the selector bytes (a Rust
//! map rather than a Ruby Hash, which saves a `VALUE` round-trip on every
//! lookup). A flood of one-off selectors - `getElementById` on unique React
//! `useId` ids, never requeried - would make every query pay for a key copy
//! and an insert that never pays back, so the hit rate is tracked over a
//! window ([`CachePolicy`]); below a floor, the cache is BYPASSED (parse per
//! call), and caching is periodically re-tested so a workload that starts
//! repeating selectors regains it.
//!
//! The policy was written for Lexbor's parser, whose lists shared one arena
//! that grew with every cached list and slowed each later parse; the port's
//! lists are owned, one allocation set per selector, so a cached entry costs
//! only its memory. The same constants are kept.

#![forbid(unsafe_code)]
// Its one entry point (`with_compiled`) is the Ruby glue's
// (`glue::html_node::css`) and the `html_css` fuzz harness's.

use std::collections::HashMap;

use crate::css_match::Scratch;
use crate::falloc::{try_to_boxed_slice, Reserve};
use crate::gvl::{Gvl, GvlCell};
use crate::html::dom::css::HtmlHost;
use crate::selectors::ast::Lists;
use crate::selectors::{Parsed, Status};

/// Why a parse did not produce a selector list.
#[derive(Debug)]
pub enum ParseError {
    /// The parser could not be created.
    NotReady,
    /// The selector is malformed.
    Syntax,
    /// A parse on this thread is still borrowing the parser.
    Busy,
    /// Out of memory parsing - not a verdict on the selector.
    Oom,
}

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

/// Selector bytes -> the parsed selector.
struct Cache {
    map: Option<HashMap<Box<[u8]>, Parsed>>,
}

impl Cache {
    const fn new() -> Self {
        Cache { map: None }
    }

    fn map(&mut self) -> &mut HashMap<Box<[u8]>, Parsed> {
        self.map.get_or_insert_with(HashMap::new)
    }

    fn flush(&mut self) {
        self.map().clear();
    }

    /// The cached parse of `selector` - which counts as a hit in `policy` -
    /// or one parsed and cached now.
    fn get(&mut self, policy: &mut CachePolicy, selector: &[u8]) -> Result<&Parsed, ParseError> {
        if self.map().contains_key(selector) {
            policy.hit();
        } else {
            /* Bound the cache: when it is full, drop every entry at once. */
            if self.map().len() >= CACHE_CAP {
                self.flush();
            }
            /* The key is copied: the borrow points into a Ruby String that may
             * be collected or mutated, while the entry outlives the call. */
            let key = try_to_boxed_slice(selector).ok_or(ParseError::Oom)?;
            let parsed = parse(selector)?;
            if self.map().falloc_reserve(1).is_err() {
                return Err(ParseError::Oom);
            }
            self.map().insert(key, parsed);
        }
        self.map().get(selector).ok_or(ParseError::Oom)
    }
}

struct Globals {
    policy: CachePolicy,
    cache: Cache,
    /// The matcher's stacks, kept from one query to the next (its doc).
    scratch: Scratch<HtmlHost>,
}

/// The one process-global, borrowed once per query.
static G: GvlCell<Globals> = GvlCell::new(Globals {
    policy: CachePolicy::new(),
    cache: Cache::new(),
    scratch: Scratch::new(),
});

/// Parse `selector` with the ported parser.
fn parse(selector: &[u8]) -> Result<Parsed, ParseError> {
    crate::selectors::parse(selector).map_err(|st| match st {
        Status::ErrorMemoryAllocation => ParseError::Oom,
        _ => ParseError::Syntax,
    })
}

/// Parse `selector` (or serve it from the cache) and hand its comma groups to
/// `f` as a [`Lists`] view - with the matcher's kept [`Scratch`].
///
/// Three ways: a window that ended with caching off flushes the cache;
/// bypassing parses per call; otherwise the selector comes from the cache,
/// parsed into it on a miss.
pub fn with_compiled<R>(
    gvl: &Gvl,
    selector: &[u8],
    f: impl FnOnce(Lists<'_>, &mut Scratch<HtmlHost>) -> R,
) -> Result<R, ParseError> {
    let mut guard = G.borrow(gvl).map_err(|_| ParseError::Busy)?;
    let g = &mut *guard;

    if g.policy.tick() {
        g.cache.flush();
    }

    if g.policy.bypassing() {
        let parsed = parse(selector)?;
        return Ok(f(parsed.groups(), &mut g.scratch));
    }

    let parsed = g.cache.get(&mut g.policy, selector)?;
    Ok(f(parsed.groups(), &mut g.scratch))
}
