//! The OLD CSS selector engine, over Lexbor's own `lxb_selectors` - kept only
//! as the reference the differential tests hold `lexbor::css_match` to
//! (`lexbor::tests::css_match`) and the `html_css_diff` fuzz harness, so it
//! is compiled into tests and the `css-reference` feature alone
//! (`lexbor/mod.rs`). `Node#css` / `#at_css` / `#matches?` run on
//! `css_match`, with the compiled-selector cache in `selector_cache`.
//!
//! The selector parser, its arena and the traversal engine are process-global,
//! each query borrowing them through a [`GvlCell`]; a query parses its
//! selector, runs it, and cleans the parser and the arena. (It used to cache
//! compiled selectors, adaptively; that policy now lives only in
//! `selector_cache`, whose module doc keeps its reasons.)
//!
//! Every Lexbor type here stays opaque: the parser's status and the setters
//! this needs are `lxb_inline`, and Lexbor publishes a `_noi` twin of each for
//! exactly this case.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::caught::PanicLatch;
use crate::falloc::VecPush;
use core::ffi::c_void;
use core::ptr::NonNull;

use crate::gvl::{Gvl, GvlCell, GvlRef};
use crate::lexbor::abi::consts::{
    STATUS_ERROR_MEMORY_ALLOCATION as LXB_STATUS_ERROR_MEMORY_ALLOCATION,
    STATUS_OK as LXB_STATUS_OK, STATUS_STOP as LXB_STATUS_STOP,
};
use crate::lexbor::abi::{
    lxb_selectors_create, lxb_selectors_destroy, lxb_selectors_find, lxb_selectors_init,
    lxb_selectors_match_node, lxb_selectors_opt_set_noi, LxbNode,
};
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::css_engine::{Owned, ParseFail, ParserParts, SelectorParser};

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
    /// Out of memory parsing the selector (`contains_guard`'s copy, or
    /// Lexbor's parser) - not a verdict on the selector.
    ParseOom,
    /// Lexbor's traversal stopped with an error that is not an allocation
    /// failure: a selector it parses but cannot run (the `||` column
    /// combinator). Reported, because the matches so far are not the answer.
    Traversal,
    /// The process-global engine could not be built.
    Unavailable,
    /// A query on this thread is still using the engine.
    Busy,
}

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
/// `Copy`, and handed out BY VALUE rather than as a reference into
/// [`Globals`], so holding it keeps no second borrow of the globals alive.
#[derive(Clone, Copy)]
struct Engine {
    parser: SelectorParser,
    selectors: *mut Selectors,
}

/// A compiled selector list in the engine's shared CSS arena, which owns it:
/// cleaning the arena invalidates it, so it is a local that goes out of scope
/// before `with_compiled_selector` cleans. Non-null, and neither `Copy` nor
/// `Clone`.
struct CompiledList(NonNull<SelectorList>);

impl CompiledList {
    /// The list a parse just produced, or why there is none.
    fn new(p: Result<*mut SelectorList, ParseFail>) -> Result<CompiledList, ParseFail> {
        p.and_then(|l| NonNull::new(l).ok_or(ParseFail::Rejected))
            .map(CompiledList)
    }

    fn as_ptr(&self) -> *const SelectorList {
        self.0.as_ptr()
    }
}

struct Globals {
    engine: Option<Engine>,
}

/// The one process-global, borrowed once per query by [`Session`].
static G: GvlCell<Globals> = GvlCell::new(Globals { engine: None });

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
 * each latch their error and STOP. The caller reports them after the traversal has
 * unwound normally and the engine has been reset. */

struct FindCtx {
    nodes: Vec<RawNode>,
    /// Excluded from the results: `css` is descendant-only, like Nokogiri's.
    root: RawNode,
    /// Why the walk stopped early: the node cap or an allocation failure.
    stopped: Option<SelectError>,
    /// A panic, latched the same way as `stopped`: it stops the walk and is
    /// reported after it, because unwinding into Lexbor would abort.
    panic: PanicLatch,
}

unsafe extern "C" fn find_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FindCtx);
    let (nodes, root, stopped) = (&mut c.nodes, c.root, &mut c.stopped);
    c.panic.guard(LXB_STATUS_STOP, || {
        /* Lexbor reports no null match; the root is not a descendant. */
        let Some(found) = RawNode::from_ptr(node.cast()).filter(|&n| n != root) else {
            return LXB_STATUS_OK;
        };
        if nodes.len() >= NODE_SET_MAX {
            *stopped = Some(SelectError::Overflow);
            return LXB_STATUS_STOP;
        }
        /* Not a bare `push`: the global allocator aborts on OOM, and this path
         * fails closed by reporting instead (`rake oom` sweeps it). */
        if nodes.falloc_push(found).is_err() {
            *stopped = Some(SelectError::CollectOom);
            return LXB_STATUS_STOP;
        }
        LXB_STATUS_OK
    })
}

struct FirstCtx {
    root: RawNode,
    found: Option<RawNode>,
    panic: PanicLatch,
}

unsafe extern "C" fn first_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FirstCtx);
    let (root, found) = (c.root, &mut c.found);
    c.panic.guard(LXB_STATUS_STOP, || {
        /* Descendant-only, so the root does not count; nor does a null. */
        match RawNode::from_ptr(node.cast()).filter(|&n| n != root) {
            Some(n) => {
                *found = Some(n);
                LXB_STATUS_STOP
            }
            None => LXB_STATUS_OK,
        }
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
    /// Run the traversal. Its status is the answer's validity: `OK`, or the
    /// `STOP` the callbacks return on purpose (`match_node` passes it back;
    /// `find` turns it into `OK`), means the walk ran as asked. Anything else
    /// means Lexbor gave up part-way - an allocation failure, or a selector it
    /// cannot run - and what the callbacks collected is a truncated answer.
    unsafe fn call(
        &self,
        e: &Engine,
        node: RawNode,
        list: &CompiledList,
        ctx: *mut c_void,
    ) -> Result<(), SelectError> {
        let (node, list) = (node.as_lxb_mut(), list.as_ptr());
        let status = match self {
            Run::Find(cb) => {
                lxb_selectors_opt_set_noi(e.selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
                lxb_selectors_find(e.selectors, node, list, Some(*cb), ctx)
            }
            Run::MatchNode(cb) => lxb_selectors_match_node(e.selectors, node, list, Some(*cb), ctx),
        };
        match status {
            LXB_STATUS_OK | LXB_STATUS_STOP => Ok(()),
            LXB_STATUS_ERROR_MEMORY_ALLOCATION => Err(SelectError::CollectOom),
            _ => Err(SelectError::Traversal),
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
/// On the ordinary path it does nothing: the query cleans after itself, and
/// this is not a substitute for that.
/// It is also what holds the borrow of the globals for the query, so the reset
/// runs under that borrow rather than taking a second one while the stack
/// unwinds.
struct Session<'g> {
    /// Held, not read: the borrow of the globals, for the whole query.
    _g: GvlRef<'g, Globals>,
    engine: Engine,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        // SAFETY: the borrow in `_g` is live, and nothing reads the list after
        // the query that is unwinding.
        unsafe { self.engine.parser.clean_all() }
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
    node: RawNode,
    run: Run,
    ctx: *mut c_void,
) -> Result<(), SelectError> {
    /* One borrow of the process-global state, held by the session for the
     * whole query. The engine comes back by value, so holding it does not keep
     * a second borrow alive. */
    let mut g = G.borrow(gvl).map_err(|_| SelectError::Busy)?;
    let e = engine_in(&mut g)?;
    let _session = Session { _g: g, engine: e };
    /* The traversal engine self-cleans; the parser and its arena are cleaned
     * here, with the list out of scope before the arena it lives in goes. */
    let ran = match CompiledList::new(e.parser.parse(selector)) {
        Ok(list) => run.call(&e, node, &list, ctx),
        Err(fail) => Err(select_error(fail)),
    };
    e.parser.clean_all();
    ran
}

/// The error a failed parse is reported as.
fn select_error(fail: ParseFail) -> SelectError {
    match fail {
        ParseFail::Rejected => SelectError::Syntax,
        ParseFail::GuardOom | ParseFail::ParserOom => SelectError::ParseOom,
    }
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
    let walked =
        unsafe { with_compiled_selector(gvl, selector, root, C::RUN, (ctx as *mut C).cast()) };
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
        root,
        stopped: None,
        panic: PanicLatch::new(),
    };
    walk(gvl, root, selector, &mut ctx)?;
    match ctx.stopped {
        Some(e) => Err(e),
        None => Ok(ctx.nodes),
    }
}

/// The first matching **descendant** of `root`, or `None`.
#[inline]
pub fn select_first(
    gvl: &Gvl,
    root: RawNode,
    selector: &[u8],
) -> Result<Option<RawNode>, SelectError> {
    let mut ctx = FirstCtx {
        root,
        found: None,
        panic: PanicLatch::new(),
    };
    walk(gvl, root, selector, &mut ctx)?;
    Ok(ctx.found)
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
