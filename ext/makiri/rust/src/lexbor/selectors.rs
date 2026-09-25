//! The CSS selector engine, over Lexbor's `lxb_selectors`: the process-global
//! parser/arena/traversal, the adaptive compiled-selector cache, and the safe
//! `select_all` / `select_first` / `matches_node` entries.
//!
//! The Ruby methods (`Node#css` / `#at_css` / `#matches?`) and the NodeSet they
//! fill live in [`crate::bridge::selectors`]; keeping them there is what stops
//! this module from depending on `bridge::html`/`bridge::node_set`, which are
//! built on top of it.
//! Every Lexbor type here stays opaque: the parser's status and the setters
//! this needs are `lxb_inline`, and Lexbor publishes a `_noi` twin of each for
//! exactly this case. So, as in `lexbor::serialize`, there is no vendored layout
//! to cross-check.
//!
//! # The engine is built once and reused
//!
//! The selector parser, its arena, and the traversal engine are process-global.
//! CSS evaluation always holds the GVL - it never releases it - so every query
//! is serialized and one shared engine needs no locking. Each entry takes the
//! [`Gvl`] that proves it, and the engine lives in a [`GvlCell`]. Creating and destroying
//! it per call (four create/init/destroy triples) dominated a cheap query like
//! `at_css('#id')`, where the match is found almost immediately and setup IS the
//! cost. Between calls only the parser returns to its CLEAN stage; the traversal
//! engine self-cleans after each find.
//!
//! # The compiled-selector cache adapts
//!
//! Parsing the selector dominates when the same one is queried repeatedly, so
//! compiled lists are cached in a map keyed by the selector bytes ([`SelectorCache`];
//! a Rust map rather than a Ruby Hash, which saves a `VALUE` round-trip on every
//! lookup - it measured ~12% of `matches?`). But
//! holding many distinct lists in the shared arena makes each new parse slower,
//! so a flood of one-off selectors - `getElementById` on unique React `useId`
//! ids, never requeried - turned the cache into a net loss (~22% slower per
//! call). The hit rate is tracked over a window ([`CachePolicy`]); below a
//! floor, the cache is BYPASSED (parse + clean per call, so the arena stays
//! small and the worst case is merely "as fast as no cache"), and caching is
//! periodically re-tested so a workload that starts repeating selectors regains
//! it.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::caught::PanicLatch;
use crate::falloc::{try_to_boxed_slice, MapInsert, Reserve, VecPush};
use core::ffi::c_void;
use std::collections::HashMap;

use crate::gvl::{Gvl, GvlCell, GvlRef};
use crate::lexbor::abi::consts::{STATUS_OK as LXB_STATUS_OK, STATUS_STOP as LXB_STATUS_STOP};
use crate::lexbor::abi::{
    lxb_selectors_create, lxb_selectors_destroy, lxb_selectors_find, lxb_selectors_init,
    lxb_selectors_match_node, lxb_selectors_opt_set_noi, LxbNode,
};
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::css_engine::{Owned, ParserParts, SelectorParser};

use crate::limits::NODE_SET_MAX;

/// Why a selector query did not produce a result. The Ruby-facing layer maps
/// each variant to its exception and message.
pub enum SelectError {
    /// The selector did not parse.
    Syntax,
    /// The result set hit `NODE_SET_MAX`.
    Overflow,
    /// Out of memory collecting the matches.
    CollectOom,
    /// Out of memory in the compiled-selector cache's bookkeeping.
    CacheOom,
    /// The process-global engine could not be built.
    Unavailable,
    /// A query on this thread is still using the engine.
    Busy,
}

const CACHE_CAP: usize = 256;
/// Re-evaluate the hit rate every N lookups.
const WIN: usize = 1024;
/// Below this hit rate (percent), bypass the cache.
const MIN_HIT_PCT: usize = 15;
/// Re-test caching every N bypass windows.
const RETEST_GAP: usize = 32;

const LXB_SELECTORS_OPT_MATCH_FIRST: crate::lexbor::abi::lxb_selectors_opt_t =
    crate::lexbor::abi::lxb_selectors_opt_t_LXB_SELECTORS_OPT_MATCH_FIRST;

/// `lxb_selectors_t`, the traversal engine. Only ever passed along.
type Selectors = crate::lexbor::abi::lxb_selectors_t;

/// The parsed selector list. The engine only passes the pointer along - it
/// reads no field.
pub type SelectorList = crate::lexbor::abi::lxb_css_selector_list_t;

type SelectorCb = unsafe extern "C" fn(*mut LxbNode, u32, *mut c_void) -> u32;

/* ------------------------------------------------------------------ */
/* process-global state                                               */
/* ------------------------------------------------------------------ */

/// The selector parser and the traversal engine, as plain pointers.
///
/// `Copy`, and handed out BY VALUE rather than as a reference into [`Globals`]:
/// that is what lets a caller hold the engine while it also touches the policy
/// and the cache, which a reference would make a second live borrow of the one
/// `Globals`.
#[derive(Clone, Copy)]
struct Engine {
    parser: SelectorParser,
    selectors: *mut Selectors,
}

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

/// Selector bytes -> the compiled list, which lives in the shared arena.
///
/// The map and the arena are only consistent together, so everything that
/// empties the arena here also empties the map: no entry can outlive the memory
/// it points into.
struct SelectorCache {
    map: Option<HashMap<Box<[u8]>, *mut SelectorList>>,
}

impl SelectorCache {
    const fn new() -> Self {
        SelectorCache { map: None }
    }

    /// The map, created on first use.
    fn map(&mut self) -> &mut HashMap<Box<[u8]>, *mut SelectorList> {
        self.map.get_or_insert_with(HashMap::new)
    }

    fn get(&mut self, selector: &[u8]) -> Option<*mut SelectorList> {
        self.map().get(selector).copied()
    }

    /// Drop every compiled list: the arena they live in and the map.
    ///
    /// # Safety
    /// The globals' borrow is live, and no cached list is used afterwards.
    unsafe fn flush(&mut self, p: SelectorParser) {
        p.clean_arena();
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
    ) -> Result<*mut SelectorList, SelectError> {
        /* Bound the cache BEFORE parsing: when it is full, drop every compiled
         * list at once, so the new list is parsed into the now-empty arena.
         * Flushing after the parse would free the very list just produced. */
        if self.map().len() >= CACHE_CAP {
            self.flush(p);
        }

        /* Prepare the owned key and reserve the map before Lexbor allocates the
         * compiled list, so a bookkeeping OOM cannot strand a live list in the
         * shared arena. The key is copied: the borrow points into a Ruby String
         * that may be collected or mutated, while the entry outlives the call. */
        let key = try_to_boxed_slice(selector).ok_or(SelectError::CacheOom)?;
        if self.map().falloc_reserve(1).is_err() {
            return Err(SelectError::CacheOom);
        }

        let list = p.parse(selector);
        /* Return the parser to its CLEAN stage, but do NOT clean the arena -
         * the list just parsed lives there and is about to be cached. */
        p.clean_parser();
        let Some(list) = list else {
            /* A rejected parse drops the cached lists instead of keeping them.
             * This belongs to the same decision as `super::contains_guard` and
             * goes with it; errors are not a hot path, so the cost is a cold
             * cache. See CLAUDE.md. */
            self.flush(p);
            return Err(SelectError::Syntax);
        };

        if self.map().falloc_insert(key, list).is_err() {
            self.flush(p);
            return Err(SelectError::CacheOom);
        }
        Ok(list)
    }
}

struct Globals {
    engine: Option<Engine>,
    policy: CachePolicy,
    cache: SelectorCache,
}

/// The one process-global, borrowed once per query by [`Session`].
static G: GvlCell<Globals> = GvlCell::new(Globals {
    engine: None,
    policy: CachePolicy::new(),
    cache: SelectorCache::new(),
});

/// Build the shared engine on first use, and hand it back by value. On failure
/// everything is torn down and the globals stay unset, so a later call retries.
fn engine_in(g: &mut Globals) -> Result<Engine, SelectError> {
    if g.engine.is_none() {
        /* Each piece is owned until the set is whole: any return below frees
         * exactly what was built, without saying so. */
        let parts = ParserParts::build().ok_or(SelectError::Unavailable)?;
        // SAFETY: Lexbor's constructor takes no Rust memory, and `init` runs on
        // exactly what it returned, owned by `selectors`.
        let selectors = unsafe {
            let s = Owned::new(lxb_selectors_create(), lxb_selectors_destroy)
                .ok_or(SelectError::Unavailable)?;
            if lxb_selectors_init(s.as_ptr()) != LXB_STATUS_OK {
                return Err(SelectError::Unavailable);
            }
            s
        };
        g.engine = Some(Engine {
            parser: parts.into_parser(),
            selectors: selectors.into_raw(),
        });
    }
    g.engine.ok_or(SelectError::Unavailable)
}

/* ------------------------------------------------------------------ */
/* the traversal callbacks                                            */
/* ------------------------------------------------------------------ */

/* These run inside Lexbor's traversal frames, so they must NOT raise: a longjmp
 * would abort the walk mid-way AND skip the engine reset, leaving the
 * process-global parser dirty for every later query. So they touch no Ruby at
 * all - matches go into a plain Vec, and the node cap and an allocation failure
 * each latch a flag and STOP. The caller reports them after the traversal has
 * unwound normally and the engine has been reset. */

struct FindCtx {
    nodes: Vec<RawNode>,
    /// Excluded from the results: `css` is descendant-only, like Nokogiri's.
    root: *mut LxbNode,
    overflow: bool,
    oom: bool,
    /// A panic, latched the same way as the two flags above: it stops the walk
    /// and is reported after it, because unwinding into Lexbor would abort.
    panic: PanicLatch,
}

unsafe extern "C" fn find_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FindCtx);
    let (nodes, root, overflow, oom) = (&mut c.nodes, c.root, &mut c.overflow, &mut c.oom);
    c.panic.guard(LXB_STATUS_STOP, || {
        if node == root {
            return LXB_STATUS_OK;
        }
        let Some(found) = RawNode::from_ptr(node.cast()) else {
            return LXB_STATUS_OK; /* Lexbor reports no null match */
        };
        if nodes.len() >= NODE_SET_MAX {
            *overflow = true;
            return LXB_STATUS_STOP;
        }
        /* Not a bare `push`: the global allocator aborts on OOM, and this path
         * fails closed by reporting instead (`rake oom` sweeps it). */
        if nodes.falloc_push_amortized(found).is_err() {
            *oom = true;
            return LXB_STATUS_STOP;
        }
        LXB_STATUS_OK
    })
}

struct FirstCtx {
    root: *mut LxbNode,
    found: *mut LxbNode,
    panic: PanicLatch,
}

unsafe extern "C" fn first_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FirstCtx);
    let (root, found) = (c.root, &mut c.found);
    c.panic.guard(LXB_STATUS_STOP, || {
        if node == root {
            return LXB_STATUS_OK; /* descendant-only */
        }
        *found = node;
        LXB_STATUS_STOP
    })
}

struct MatchCtx {
    matched: bool,
    panic: PanicLatch,
}

unsafe extern "C" fn match_cb(_node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut MatchCtx);
    let matched = &mut c.matched;
    c.panic.guard(LXB_STATUS_STOP, || {
        *matched = true;
        LXB_STATUS_STOP
    })
}

/* ------------------------------------------------------------------ */
/* compile + run                                                      */
/* ------------------------------------------------------------------ */

/// What to do with a compiled selector list.
enum Run {
    /// Collect every matching descendant. `MATCH_FIRST` dedups a node that
    /// matches several selectors of a comma list.
    Find(SelectorCb),
    /// Test this one node.
    MatchNode(SelectorCb),
}

impl Run {
    unsafe fn call(
        &self,
        e: &Engine,
        node: *mut LxbNode,
        list: *const SelectorList,
        ctx: *mut c_void,
    ) {
        match self {
            Run::Find(cb) => {
                lxb_selectors_opt_set_noi(e.selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
                lxb_selectors_find(e.selectors, node, list, Some(*cb), ctx);
            }
            Run::MatchNode(cb) => {
                lxb_selectors_match_node(e.selectors, node, list, Some(*cb), ctx);
            }
        }
    }
}

/// Returns the process-global engine to a known-good state IF the stack is
/// unwinding past it.
///
/// The engine outlives every call, so a panic in the middle of one would leave
/// the shared parser in a non-CLEAN stage and a half-parsed list in the shared
/// arena - for every LATER query, not just the one that failed. The crate
/// unwinds (`panic = "unwind"`, so magnus turns a panic into a Ruby exception),
/// and a plain statement after the parse is exactly what unwinding skips.
///
/// It resets the parser, the arena and the cache as a unit, because they are
/// only consistent together. The cost is one cold cache after a panic, which is
/// the right trade for a process-global.
///
/// On the ordinary path it does nothing: each `clean` below has its own meaning
/// and its own place, and this is not a substitute for them.
/// It is also what holds the borrow of the globals for the query, so the reset
/// uses that borrow rather than taking a second one while the stack unwinds.
struct Session<'g> {
    g: GvlRef<'g, Globals>,
    engine: Engine,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let parser = self.engine.parser;
        // SAFETY: the borrow in `g` is live, and nothing reads a cached list
        // after the query that is unwinding.
        unsafe {
            self.g.cache.flush(parser);
            parser.clean_parser();
        }
    }
}

/// Parse `selector` with the shared engine, hand the compiled list to `run`,
/// then leave the engine ready for the next call.
///
/// A syntax error is *returned*: magnus raises it after this function has
/// returned normally, so the reset below is plain control flow rather than
/// something an error path has to remember. A PANIC is the case that is not
/// plain control flow, and [`Session`] covers it.
unsafe fn with_compiled_selector(
    gvl: &Gvl,
    selector: &[u8],
    node: *mut LxbNode,
    run: Run,
    ctx: *mut c_void,
) -> Result<(), SelectError> {
    /* One borrow of the process-global state, held by the session for the
     * whole query. The engine comes back by value, so holding it does not keep
     * a second borrow alive. */
    let mut g = G.borrow(gvl).map_err(|_| SelectError::Busy)?;
    let e = engine_in(&mut g)?;
    let mut session = Session { g, engine: e };
    let g = &mut *session.g;

    if g.policy.tick() {
        g.cache.flush(e.parser);
    }

    if g.policy.bypassing() {
        /* Parse + clean per call - the behaviour before the cache existed - so
         * the arena stays small and a one-off-selector flood is no slower than
         * having no cache at all. */
        let list = e.parser.parse(selector);
        if let Some(list) = list {
            run.call(&e, node, list, ctx);
        }
        e.parser.clean_all();
        return list.map(|_| ()).ok_or(SelectError::Syntax);
    }

    let list = match g.cache.get(selector) {
        Some(list) => {
            g.policy.hit();
            list
        }
        None => g.cache.compile(e.parser, selector)?,
    };
    /* The traversal engine self-cleans; the cached list and its arena stay. */
    run.call(&e, node, list, ctx);
    Ok(())
}

/* ------------------------------------------------------------------ */
/* the safe entries the Ruby layer calls                              */
/* ------------------------------------------------------------------ */

/// A traversal's context, tied to the callback that reads it - so the pairing
/// the `*mut c_void` hand-off relies on is made once, by type, not per call.
trait Walk {
    /// What to run; its callback casts the context back to `Self`.
    const RUN: Run;
    fn latch(&mut self) -> &mut PanicLatch;
}

impl Walk for FindCtx {
    const RUN: Run = Run::Find(find_cb);
    fn latch(&mut self) -> &mut PanicLatch {
        &mut self.panic
    }
}

impl Walk for FirstCtx {
    const RUN: Run = Run::Find(first_cb);
    fn latch(&mut self) -> &mut PanicLatch {
        &mut self.panic
    }
}

impl Walk for MatchCtx {
    const RUN: Run = Run::MatchNode(match_cb);
    fn latch(&mut self) -> &mut PanicLatch {
        &mut self.panic
    }
}

/// Run `C`'s traversal of `selector` from `root` into `ctx`, re-raising a
/// panic the callback latched.
fn walk<C: Walk>(
    gvl: &Gvl,
    root: RawNode,
    selector: &[u8],
    ctx: &mut C,
) -> Result<(), SelectError> {
    // SAFETY: `root` is a live node whose document outlives the call, and
    // `C::RUN`'s callback reads the context as the `C` it is.
    let walked = unsafe {
        with_compiled_selector(
            gvl,
            selector,
            root.as_ptr() as *mut LxbNode,
            C::RUN,
            (ctx as *mut C).cast(),
        )
    };
    /* Before the caller's `?`: Lexbor has unwound and the engine is reset, so
     * this is the first frame where re-raising is safe. */
    ctx.latch().resume();
    walked
}

/// Every matching **descendant** of `root` (the context node itself excluded),
/// in document order. `Err` for a bad selector, the node cap or OOM.
#[inline]
pub fn select_all(gvl: &Gvl, root: RawNode, selector: &[u8]) -> Result<Vec<RawNode>, SelectError> {
    let mut ctx = FindCtx {
        nodes: Vec::new(),
        root: root.as_ptr() as *mut LxbNode,
        overflow: false,
        oom: false,
        panic: PanicLatch::new(),
    };
    walk(gvl, root, selector, &mut ctx)?;
    if ctx.overflow {
        return Err(SelectError::Overflow);
    }
    if ctx.oom {
        return Err(SelectError::CollectOom);
    }
    Ok(ctx.nodes)
}

/// The first matching **descendant** of `root`, or `None`.
#[inline]
pub fn select_first(
    gvl: &Gvl,
    root: RawNode,
    selector: &[u8],
) -> Result<Option<RawNode>, SelectError> {
    let mut ctx = FirstCtx {
        root: root.as_ptr() as *mut LxbNode,
        found: core::ptr::null_mut(),
        panic: PanicLatch::new(),
    };
    walk(gvl, root, selector, &mut ctx)?;
    Ok(RawNode::from_ptr(ctx.found.cast()))
}

/// Does `root` itself match `selector`?
#[inline]
pub fn matches_node(gvl: &Gvl, root: RawNode, selector: &[u8]) -> Result<bool, SelectError> {
    let mut ctx = MatchCtx {
        matched: false,
        panic: PanicLatch::new(),
    };
    walk(gvl, root, selector, &mut ctx)?;
    Ok(ctx.matched)
}
