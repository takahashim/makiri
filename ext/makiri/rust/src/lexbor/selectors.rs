//! CSS selector queries (glue/ruby_html_css.c), over Lexbor's `lxb_selectors`.
//!
//!   `Node#css(selector)`    -> NodeSet of matching descendants, document order
//!   `Node#at_css(selector)` -> the first matching descendant, or nil
//!   `Node#matches?(sel)`    -> does THIS node match (like Nokogiri)
//!
//! Every Lexbor type here stays opaque: the parser's status and the two setters
//! this needs are `lxb_inline`, and Lexbor publishes a `_noi` twin of each for
//! exactly this case. So, as in `lexbor::serialize`, there is no vendored layout
//! to cross-check.
//!
//! # The engine is built once and reused
//!
//! The selector parser, its arena, and the traversal engine are process-global.
//! CSS evaluation always holds the GVL - it never releases it - so every query
//! is serialized and one shared engine needs no locking. Creating and destroying
//! it per call (four create/init/destroy triples) dominated a cheap query like
//! `at_css('#id')`, where the match is found almost immediately and setup IS the
//! cost. Between calls only the parser returns to its CLEAN stage; the traversal
//! engine self-cleans after each find.
//!
//! # The compiled-selector cache adapts
//!
//! Parsing the selector dominates when the same one is queried repeatedly, so
//! compiled lists are cached in a map keyed by the selector bytes. (The C used a
//! Ruby Hash storing the list's address as an Integer; a Rust map holds the same
//! thing without a `VALUE` round-trip on every lookup, which is worth having on
//! a path this hot - it measured ~12% of `matches?`.) But
//! holding many distinct lists in the shared arena makes each new parse slower,
//! so a flood of one-off selectors - `getElementById` on unique React `useId`
//! ids, never requeried - turned the cache into a net loss (~22% slower per
//! call). The hit rate is tracked over a window; below a floor, the cache is
//! BYPASSED (parse + clean per call, so the arena stays small and the worst case
//! is merely "as fast as no cache"), and caching is periodically re-tested so a
//! workload that starts repeating selectors regains it.

#![allow(unsafe_code)]
/* Every function takes the `VALUE`s its caller already holds. */
#![allow(clippy::missing_safety_doc)]

use crate::falloc::{try_to_boxed_slice, MapInsert, Reserve};
use core::cell::UnsafeCell;
use core::ffi::{c_int, c_void};
use std::collections::HashMap;

use magnus::rb_sys::AsRawValue;
use magnus::{method, prelude::*, Error, Ruby, Value};
use crate::bridge::ruby::VALUE;

use crate::bridge::node_set::{node_set_new, node_set_push, PushError};
use crate::bridge::lexbor::{html_node_unwrap, keepalive_document, wrap_html_node};
use crate::bridge::ruby::error_class;
use crate::bridge::string::{ruby_bytes_view, verify_text};
use crate::lexbor::ffi::{LxbNode, LXB_STATUS_OK};
use crate::lexbor::adapter::html::RawNode;
use crate::init::{EXC_CSS_SYNTAX_ERROR, MOD_HTML_NODE_METHODS};

/// Mirrors `NODE_SET_MAX`: every node-collecting path fails closed at the
/// same bound.
const NODE_SET_MAX: usize = 10 * 1000 * 1000;

const CACHE_CAP: usize = 256;
/// Re-evaluate the hit rate every N lookups.
const WIN: usize = 1024;
/// Below this hit rate (percent), bypass the cache.
const MIN_HIT_PCT: usize = 15;
/// Re-test caching every N bypass windows.
const RETEST_GAP: usize = 32;

use crate::lexbor_abi::consts::STATUS_STOP as LXB_STATUS_STOP;
const LXB_SELECTORS_OPT_MATCH_FIRST: u32 = 1 << 2;

/* ---- opaque Lexbor types ---- */

macro_rules! opaque {
    ($($name:ident),* $(,)?) => {$(
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
        }
    )*};
}
opaque!(Selectors);

/// The parsed selector list. Aliased to the generated type rather than kept
/// opaque here: `lxb_css_selectors_parse` is declared once, in `lexbor_abi`, and
/// a second opaque spelling gave that symbol two Rust types. The engine still
/// only passes the pointer along - it reads no field.
pub type SelectorList = crate::lexbor_abi::lxb_css_selector_list_t;

/* The CSS memory arena and selector table are declared in `lexbor_abi` along
 * with the parser, so the Ruby-free lowering can reach the same ones. */
pub use crate::lexbor_abi::{
    lxb_css_memory_clean, lxb_css_memory_create, lxb_css_memory_destroy, lxb_css_memory_init,
    lxb_css_parser_memory_set_noi, lxb_css_parser_selectors_set_noi, lxb_css_parser_status_noi,
    lxb_css_selectors_create, lxb_css_selectors_destroy, lxb_css_selectors_init,
    lxb_css_selectors_parse, CssMemory, CssSelectors,
};

/// The parser is declared in `lexbor_abi` - see the note there.
use crate::lexbor::ffi::{
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    CssParser,
};

type SelectorCb = unsafe extern "C" fn(*mut LxbNode, u32, *mut c_void) -> u32;

extern "C" {

    /// The `_noi` twins of Lexbor's `lxb_inline` accessors.
    fn lxb_selectors_create() -> *mut Selectors;
    fn lxb_selectors_init(s: *mut Selectors) -> u32;
    fn lxb_selectors_destroy(s: *mut Selectors, self_destroy: bool) -> *mut Selectors;
    fn lxb_selectors_opt_set_noi(s: *mut Selectors, opt: u32);
    fn lxb_selectors_find(
        s: *mut Selectors,
        root: *mut LxbNode,
        list: *const SelectorList,
        cb: SelectorCb,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_selectors_match_node(
        s: *mut Selectors,
        node: *mut LxbNode,
        list: *const SelectorList,
        cb: SelectorCb,
        ctx: *mut c_void,
    ) -> u32;
}

/* ------------------------------------------------------------------ */
/* process-global state                                               */
/* ------------------------------------------------------------------ */

/// A `static mut` in all but name, sound because everything that touches it
/// holds the GVL. The C had the same globals with the same justification; this
/// spells the assumption out in one place instead of leaving it to the comments.
struct GvlCell<T>(UnsafeCell<T>);

// SAFETY: only ever reached from a Ruby thread holding the GVL, which
// serialises every access.
unsafe impl<T> Sync for GvlCell<T> {}

/// A Lexbor object owned only while the engine is being built.
///
/// The engine lives for the process - nothing frees it on the normal path, and
/// that reuse is what makes `at_css` fast - but a half-built one must not leak.
/// Each piece is owned by one of these until all four are ready; `into_raw`
/// then hands the pointer to [`Engine`] and this stops owning it.
///
/// So the unwinding that four `if !x.is_null()` arms used to do - in an order
/// that no longer matched the order the four were created in - is the type's
/// job, and a fifth piece cannot be left out of it.
struct Building<T>(*mut T, unsafe extern "C" fn(*mut T, bool) -> *mut T);

impl<T> Building<T> {
    /// `None` when Lexbor could not allocate the object.
    fn new(
        p: *mut T,
        destroy: unsafe extern "C" fn(*mut T, bool) -> *mut T,
    ) -> Option<Building<T>> {
        (!p.is_null()).then_some(Building(p, destroy))
    }

    #[inline]
    fn as_ptr(&self) -> *mut T {
        self.0
    }

    /// Hand the pointer on; this stops owning it.
    fn into_raw(self) -> *mut T {
        core::mem::ManuallyDrop::new(self).0
    }
}

impl<T> Drop for Building<T> {
    fn drop(&mut self) {
        // SAFETY: this owns the object, and only gets here when the engine was
        // not built - nothing else holds the pointer.
        unsafe { (self.1)(self.0, true) };
    }
}

struct Engine {
    mem: *mut CssMemory,
    parser: *mut CssParser,
    /// Handed to the parser once at init and never read again, but owned here:
    /// the engine lives for the process, and this is what says so.
    #[allow(dead_code)]
    css_sel: *mut CssSelectors,
    selectors: *mut Selectors,
}

struct Globals {
    engine: Option<Engine>,
    /// Selector bytes -> the compiled list, which lives in the shared arena.
    /// Cleared as a unit whenever that arena is cleaned, so no entry can
    /// outlive the memory it points into.
    cache: Option<HashMap<Box<[u8]>, *mut SelectorList>>,
    win: usize,
    win_hits: usize,
    bypass: bool,
    bypass_runs: usize,
}

static G: GvlCell<Globals> = GvlCell(UnsafeCell::new(Globals {
    engine: None,
    cache: None,
    win: 0,
    win_hits: 0,
    bypass: false,
    bypass_runs: 0,
}));

/// # Safety
/// GVL held.
unsafe fn globals() -> &'static mut Globals {
    &mut *G.0.get()
}

/// Build the shared engine on first use. On failure everything is torn down and
/// the globals stay unset, so a later call retries.
unsafe fn engine() -> Result<&'static Engine, Error> {
    let g = globals();
    if g.engine.is_none() {
        let refused = || Error::new(error_class(), "failed to initialise CSS selector engine");

        /* Each piece is owned from here until all four are whole: any return
         * below frees exactly what was built, without saying so. */
        let (Some(mem), Some(parser), Some(css_sel), Some(selectors)) = (
            Building::new(lxb_css_memory_create(), lxb_css_memory_destroy),
            Building::new(lxb_css_parser_create(), lxb_css_parser_destroy),
            Building::new(lxb_css_selectors_create(), lxb_css_selectors_destroy),
            Building::new(lxb_selectors_create(), lxb_selectors_destroy),
        ) else {
            return Err(refused());
        };

        if lxb_css_memory_init(mem.as_ptr(), 128) != LXB_STATUS_OK
            || lxb_css_parser_init(parser.as_ptr(), core::ptr::null_mut()) != LXB_STATUS_OK
            || lxb_css_selectors_init(css_sel.as_ptr()) != LXB_STATUS_OK
            || lxb_selectors_init(selectors.as_ptr()) != LXB_STATUS_OK
        {
            return Err(refused());
        }

        lxb_css_parser_memory_set_noi(parser.as_ptr(), mem.as_ptr());
        lxb_css_parser_selectors_set_noi(parser.as_ptr(), css_sel.as_ptr());
        g.engine = Some(Engine {
            mem: mem.into_raw(),
            parser: parser.into_raw(),
            css_sel: css_sel.into_raw(),
            selectors: selectors.into_raw(),
        });
    }
    Ok(g.engine.as_ref().expect("just set"))
}

/// The compiled-selector cache, created on first use.
unsafe fn cache() -> &'static mut HashMap<Box<[u8]>, *mut SelectorList> {
    globals().cache.get_or_insert_with(HashMap::new)
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
    nodes: Vec<*mut LxbNode>,
    /// Excluded from the results: `css` is descendant-only, like Nokogiri's.
    root: *mut LxbNode,
    overflow: bool,
    oom: bool,
}

unsafe extern "C" fn find_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FindCtx);
    if node == c.root {
        return LXB_STATUS_OK;
    }
    if c.nodes.len() >= NODE_SET_MAX {
        c.overflow = true;
        return LXB_STATUS_STOP;
    }
    /* `try_reserve` rather than relying on `push`: the global allocator aborts
     * on OOM, and this path fails closed by reporting instead (`rake oom`
     * sweeps it). */
    if c.nodes.len() == c.nodes.capacity() && c.nodes.mkr_reserve(1).is_err() {
        c.oom = true;
        return LXB_STATUS_STOP;
    }
    c.nodes.push(node);
    LXB_STATUS_OK
}

struct FirstCtx {
    root: *mut LxbNode,
    found: *mut LxbNode,
}

unsafe extern "C" fn first_cb(node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut FirstCtx);
    if node == c.root {
        return LXB_STATUS_OK; /* descendant-only */
    }
    c.found = node;
    LXB_STATUS_STOP
}

unsafe extern "C" fn match_cb(_node: *mut LxbNode, _spec: u32, ctx: *mut c_void) -> u32 {
    *(ctx as *mut bool) = true;
    LXB_STATUS_STOP
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
                lxb_selectors_find(e.selectors, node, list, *cb, ctx);
            }
            Run::MatchNode(cb) => {
                lxb_selectors_match_node(e.selectors, node, list, *cb, ctx);
            }
        }
    }
}

/// Parse `selector` with the shared engine, hand the compiled list to `run`,
/// then leave the engine ready for the next call.
///
/// Unlike the C, a syntax error is *returned*: magnus raises it after this
/// function has returned normally, so the reset below is plain control flow
/// rather than something an error path has to remember.
unsafe fn with_compiled_selector(
    selector: Value,
    node: *mut LxbNode,
    run: Run,
    ctx: *mut c_void,
) -> Result<(), Error> {
    /* `Err` for a NUL byte or invalid UTF-8, naming the argument as the C did. */
    verify_text(selector, c"CSS selector")?;
    /* The borrow comes from the bridge rather than from RSTRING here, so the
     * view anchors the String while its bytes are read. `ruby_verified_text`
     * would do both in one, but it also coerces, and these entries are hot
     * enough that the extra type check measured. */
    let sv = ruby_bytes_view(selector.as_raw());
    let e = engine()?;
    let g = globals();

    /* The adaptive window: every WIN lookups, decide whether caching pays. */
    g.win += 1;
    if g.win > WIN {
        if !g.bypass {
            if g.win_hits * 100 < WIN * MIN_HIT_PCT {
                g.bypass = true;
                g.bypass_runs = 0;
                lxb_css_memory_clean(e.mem); /* drop the cached lists' arena */
                cache().clear();
            }
        } else {
            g.bypass_runs += 1;
            if g.bypass_runs >= RETEST_GAP {
                g.bypass = false; /* re-test caching over the next window */
            }
        }
        g.win = 1;
        g.win_hits = 0;
    }

    /* The pointer Lexbor gets is the String's own, as before - `bytes()` would
     * hand it a dangling one for an empty selector, which the cache key below
     * does not care about but a parser might. */
    let (ptr, len) = (sv.as_ptr() as *const u8, sv.len());

    if g.bypass {
        /* Parse + clean per call - the behaviour before the cache existed - so
         * the arena stays small and a one-off-selector flood is no slower than
         * having no cache at all. */
        let list = lxb_css_selectors_parse(e.parser, ptr, len);
        let bad = list.is_null() || lxb_css_parser_status_noi(e.parser) != LXB_STATUS_OK;
        if !bad {
            run.call(e, node, list, ctx);
        }
        lxb_css_memory_clean(e.mem);
        lxb_css_parser_clean(e.parser);
        return if bad {
            Err(syntax_error(selector))
        } else {
            Ok(())
        };
    }

    let key = sv.bytes();
    let h = cache();
    if let Some(&list) = h.get(key) {
        g.win_hits += 1;
        run.call(e, node, list, ctx);
        /* The traversal engine self-cleans; the cached list and its arena stay. */
        return Ok(());
    }

    /* A miss. Bound the cache BEFORE parsing: when it is full, drop every
     * compiled list at once by cleaning the shared arena, so the new list is
     * parsed into the now-empty arena. Cleaning after the parse would
     * invalidate the very list just produced. */
    if h.len() >= CACHE_CAP {
        lxb_css_memory_clean(e.mem);
        h.clear();
    }

    /* Prepare the owned key and reserve the map before Lexbor allocates the
     * compiled list. A cache bookkeeping OOM therefore cannot leave a live
     * Lexbor list stranded in the shared arena. */
    let owned_key = match try_to_boxed_slice(key) {
        Some(k) => k,
        None => {
            return Err(Error::new(
                error_class(),
                "out of memory caching CSS selector",
            ));
        }
    };
    if h.mkr_reserve(1).is_err() {
        return Err(Error::new(
            error_class(),
            "out of memory caching CSS selector",
        ));
    }

    let list = lxb_css_selectors_parse(e.parser, ptr, len);
    let bad = list.is_null() || lxb_css_parser_status_noi(e.parser) != LXB_STATUS_OK;
    /* Return the parser to its CLEAN stage, but do NOT clean the arena - the
     * list just parsed lives there and is about to be cached. */
    lxb_css_parser_clean(e.parser);
    if bad {
        return Err(syntax_error(selector));
    }

    /* The key is copied: the borrow points into a Ruby String that may be
     * collected or mutated, while the entry has to outlive the call. */
    if h.mkr_insert(owned_key, list).is_err() {
        lxb_css_memory_clean(e.mem);
        return Err(Error::new(
            error_class(),
            "out of memory caching CSS selector",
        ));
    }
    run.call(e, node, list, ctx);
    Ok(())
}

/// The C's message, exactly.
///
/// `%" PRIsVALUE` interpolates a String with `to_s`, not `inspect`, so the C
/// wrote the selector bare where `inspect` adds quotes and escapes. This used
/// `inspect` and produced `invalid CSS selector: "p:hover"` where every prior
/// release said `invalid CSS selector: p:hover` - a user-visible change that no
/// spec asserted, found by the CSS differential when the lowering was ported.
fn syntax_error(selector: Value) -> Error {
    let class = magnus::ExceptionClass::from_value(EXC_CSS_SYNTAX_ERROR.value())
        .expect("Makiri::CSS::SyntaxError");
    let shown = selector.to_string();
    Error::new(class, format!("invalid CSS selector: {shown}"))
}

/* ------------------------------------------------------------------ */
/* the Ruby methods                                                   */
/* ------------------------------------------------------------------ */

/// The arguments to the fill loop below, passed through `rb_protect`'s one
/// `VALUE`-sized slot.
struct Fill<'a> {
    set: VALUE,
    nodes: &'a [*mut LxbNode],
    /// A push the set refused, carried out of `rb_protect` for the caller.
    refused: Option<PushError>,
}

/// Move the collected matches into the NodeSet. Runs under `rb_protect`: a push
/// can raise (Ruby's allocator), and a longjmp straight out of here would skip
/// the collection Vec's drop in the caller.
unsafe extern "C" fn fill_thunk(arg: VALUE) -> VALUE {
    let f = &mut *(arg as *mut Fill);
    for n in f.nodes {
        if let Err(e) = node_set_push(f.set, *n as *mut c_void) {
            f.refused = Some(e);
            break;
        }
    }
    crate::bridge::ruby::nil().as_raw()
}

/// `Node#css`: every matching descendant, in document order.
fn css(rb_self: Value, selector: Value) -> Result<Value, Error> {
    let root = html_node_unwrap(rb_self)?.as_ptr() as *mut LxbNode;
    let document = keepalive_document(rb_self)?;

    let mut ctx = FindCtx {
        nodes: Vec::new(),
        root,
        overflow: false,
        oom: false,
    };
    unsafe {
        with_compiled_selector(
            selector,
            root,
            Run::Find(find_cb),
            &mut ctx as *mut FindCtx as *mut c_void,
        )?;
    }
    if ctx.overflow {
        return Err(Error::new(
            error_class(),
            format!("CSS result set exceeded the node limit ({NODE_SET_MAX})"),
        ));
    }
    if ctx.oom {
        return Err(Error::new(
            error_class(),
            "out of memory collecting CSS results",
        ));
    }

    let set = node_set_new(document);
    /* Each push can raise (NoMemoryError from Ruby's allocator), and a longjmp
     * would skip `ctx.nodes`'s drop. `protect` turns that into an Err, the Vec
     * drops on the way out, and magnus raises afterwards - the Rust form of the
     * C's rb_ensure, at one setjmp per call rather than per node. */
    let mut fill = Fill {
        set: set.as_raw(),
        nodes: &ctx.nodes,
        refused: None,
    };
    let fill_ptr = &mut fill as *mut Fill as VALUE;
    crate::bridge::ruby::protect_value(|| unsafe { fill_thunk(fill_ptr) })?;
    if let Some(e) = fill.refused.take() {
        return Err(e.into());
    }
    Ok(set)
}

/// `Node#at_css`: the first matching descendant, or nil.
///
/// Stops at the first match and wraps that one node - no NodeSet, and no Ruby
/// `#first` dispatch, for the single node the caller asked for.
fn at_css(rb_self: Value, selector: Value) -> Result<Value, Error> {
    let ruby = Ruby::get_with(rb_self);
    let root = html_node_unwrap(rb_self)?.as_ptr() as *mut LxbNode;

    let mut ctx = FirstCtx {
        root,
        found: core::ptr::null_mut(),
    };
    unsafe {
        with_compiled_selector(
            selector,
            root,
            Run::Find(first_cb),
            &mut ctx as *mut FirstCtx as *mut c_void,
        )?;
    }
    if ctx.found.is_null() {
        return Ok(ruby.qnil().as_value());
    }
    let document = keepalive_document(rb_self)?;
    Ok(wrap_html_node(RawNode::from_ptr(ctx.found.cast()).expect("first match"), document))
}

/// `Node#matches?`: does THIS node match? Tested against the node itself, not
/// its descendants, like Nokogiri.
fn matches(rb_self: Value, selector: Value) -> Result<bool, Error> {
    let node = html_node_unwrap(rb_self)?.as_ptr() as *mut LxbNode;
    let mut matched = false;
    unsafe {
        with_compiled_selector(
            selector,
            node,
            Run::MatchNode(match_cb),
            &mut matched as *mut bool as *mut c_void,
        )?;
    }
    Ok(matched)
}

/// # Safety
/// Called from `Init_makiri`.
pub fn init_css() {
    let m = magnus::RModule::from_value(MOD_HTML_NODE_METHODS.value())
        .expect("Makiri::HTML::NodeMethods");
    m.define_method("css", method!(css, 1)).expect("Node#css");
    m.define_method("at_css", method!(at_css, 1))
        .expect("Node#at_css");
    m.define_method("matches?", method!(matches, 1))
        .expect("Node#matches?");
    let _: Option<c_int> = None;
}
