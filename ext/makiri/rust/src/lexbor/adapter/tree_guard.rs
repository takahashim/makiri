//! The guard every HTML parse runs under: a tokenizer hook that stops the
//! parse on the two input shapes Lexbor's tree construction handles in
//! quadratic time. The parse runs with the GVL released and cannot be
//! interrupted, so either is a denial of service on untrusted input.
//!
//! - **Depth.** Most start tags walk the stack of open elements for a scope
//!   check, so `"<div>" * 80_000` (400 KB) took 5 s. The limit is Nokogiri's
//!   `max_tree_depth` (default 400) with its boundary: an element's depth
//!   counts itself and its ancestors, `<html>` being 1 in a document and the
//!   top level 1 in a fragment (whose parser keeps a synthetic `<html>` below
//!   it - [`TokenHook::new`]'s `synthetic`). Checked on the open-element stack
//!   after each token; a token that pushes several elements is bounded by what
//!   the previous check accepted.
//! - **Options per select.** Each inserted `<option>` re-runs the select's
//!   selectedness algorithm over all its options (Lexbor `9c841a3`, v3.0.0),
//!   so 40,000 options took 4 s. Counted per select, up to
//!   [`MAX_SELECT_OPTIONS`].
//!
//! The hook stops a parse by returning NULL for the token - how Lexbor's own
//! tree builder fails - and records why ([`TokenHook::stopped`]).
//!
//! It is a plain value on the caller's stack, installed on every parse, and
//! allocates nothing. The source-position [`Recorder`] rides inside it, but its
//! failures (allocation, token cap, a caught panic) only stop the recording:
//! the guard does not depend on it.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::caught::PanicLatch;
use crate::lexbor::abi as lxb;

use super::html::{HtmlNode, NsId, RawNode, TagId};
use super::source_loc::Recorder;

type Token = lxb::lxb_html_token_t;
type Tokenizer = lxb::lxb_html_tokenizer_t;
type TokenFn = lxb::lxb_html_tokenizer_token_f;
type Tree = lxb::lxb_html_tree_t;

/// The most `<option>`s one `<select>` may receive during a parse; real lists
/// are a few hundred at most.
pub const MAX_SELECT_OPTIONS: usize = 10_000;

/// What stopped a parse the hook refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardStop {
    /// The tree grew deeper than the [`DepthLimit`] allowed.
    TooDeep,
    /// One `<select>` received more than [`MAX_SELECT_OPTIONS`] options.
    TooManyOptions,
}

/// The deepest element an HTML parse accepts, or no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthLimit(Option<usize>);

impl DepthLimit {
    /// Nokogiri::HTML5's default (`Nokogiri::Gumbo::DEFAULT_MAX_TREE_DEPTH`).
    pub const DEFAULT: DepthLimit = DepthLimit(Some(400));
    /// No limit: the quadratic worst case is back.
    pub const UNLIMITED: DepthLimit = DepthLimit(None);

    /// Accept elements up to depth `depth` (see the module doc for the count).
    pub const fn at_most(depth: usize) -> DepthLimit {
        DepthLimit(Some(depth))
    }

    /// The limit, or `None` when there is none.
    pub fn max_depth(self) -> Option<usize> {
        self.0
    }

    /// The longest open-element stack allowed, given `synthetic` entries the
    /// parser keeps below the first real element.
    fn max_open(self, synthetic: usize) -> usize {
        self.0.map_or(usize::MAX, |d| d.saturating_add(synthetic))
    }
}

/// The tokenizer's token-done hook: runs the depth guard after the tree
/// builder, and records source positions for a document parse.
///
/// Installed with its own address as the callback context, so it must stay
/// where it is, and alive, from [`install`](Self::install) until the parse
/// call has returned.
pub struct TokenHook {
    /// The parser's OWN token-done callback, which builds the tree.
    delegate: TokenFn,
    delegate_ctx: *mut c_void,
    /// The tree builder whose stack of open elements is measured.
    tree: *const Tree,
    /// The stack length above which the parse stops; `usize::MAX` for none.
    max_open: usize,
    /// The `<select>` the last counted option went into, and how many have.
    /// One is enough: the parser only ever inserts into the select that is
    /// open, and once closed a select receives no more.
    select: *const c_void,
    options: usize,
    stopped: Option<GuardStop>,
    /// The document parse's position recorder; `None` for a fragment.
    recorder: Option<Recorder>,
    /// A panic in the recorder, latched rather than raised: this runs from
    /// Lexbor's tokenizer, and unwinding into C aborts. The caller raises it
    /// once the parse has returned (see `crate::caught`).
    panic: PanicLatch,
}

impl TokenHook {
    /// A hook enforcing `limit`, where the parser keeps `synthetic` entries on
    /// its stack below the first real element (0 for a document, 1 for a
    /// fragment's `<html>` root).
    pub fn new(limit: DepthLimit, synthetic: usize, recorder: Option<Recorder>) -> TokenHook {
        TokenHook {
            delegate: None,
            delegate_ctx: core::ptr::null_mut(),
            tree: core::ptr::null(),
            max_open: limit.max_open(synthetic),
            select: core::ptr::null(),
            options: 0,
            stopped: None,
            recorder,
            panic: PanicLatch::new(),
        }
    }

    /// Install on `parser`'s tokenizer, CHAINING the tree builder's own
    /// callback. `false` when the parser has no tree to measure - which an
    /// initialised parser always has - and the caller fails the parse rather
    /// than run it unguarded.
    ///
    /// # Safety
    /// `parser` must be live and initialised. `self` must not move, and must
    /// outlive every token the parser processes from here on - in practice,
    /// until the parse call that follows has returned.
    pub unsafe fn install(&mut self, parser: *mut lxb::lxb_html_parser_t) -> bool {
        let tree = lxb::lxb_html_parser_tree_noi(parser);
        if tree.is_null() {
            return false;
        }
        self.tree = tree;
        let tkz = lxb::lxb_html_parser_tokenizer_noi(parser);
        /* Lexbor has a setter and a ctx getter for the token-done callback but
         * no getter for the callback FUNCTION, so that one field is read from
         * the struct directly; the ctx uses the public accessor. */
        self.delegate = (*tkz).callback_token_done;
        self.delegate_ctx = lxb::lxb_html_tokenizer_callback_token_done_ctx_noi(tkz);
        lxb::lxb_html_tokenizer_callback_token_done_set_noi(
            tkz,
            Some(hook_token_cb),
            self as *mut TokenHook as *mut c_void,
        );
        true
    }

    /// What stopped the parse, if the hook did.
    pub fn stopped(&self) -> Option<GuardStop> {
        self.stopped
    }

    /// Re-raise a panic the recorder caught, now that Lexbor's frames are
    /// gone. A no-op when nothing panicked.
    pub fn resume_panic(&mut self) {
        self.panic.resume();
    }

    /// The recorder, once the parse is over.
    pub fn into_recorder(self) -> Option<Recorder> {
        self.recorder
    }

    /// The tree builder's open-element count.
    ///
    /// # Safety
    /// `self.tree` must be the live tree `install` found.
    #[inline]
    unsafe fn open_elements(&self) -> usize {
        let stack = (*self.tree).open_elements;
        if stack.is_null() {
            0 /* no stack, nothing open: an initialised tree always has one */
        } else {
            (*stack).length
        }
    }

    /// The element the tree builder opened last - the current node.
    ///
    /// # Safety
    /// As [`open_elements`](Self::open_elements); the stack's entries are the
    /// tree's live element nodes.
    unsafe fn current_node(&self) -> Option<RawNode> {
        let stack = (*self.tree).open_elements;
        if stack.is_null() || (*stack).length == 0 {
            return None;
        }
        // SAFETY: `length` entries of `list` are the open elements, and the
        // stack is not empty, so the last index is in bounds.
        RawNode::from_ptr(*(*stack).list.add((*stack).length - 1))
    }

    /// Count an `<option>` the tree builder just inserted, against the select
    /// it updates; whether that select is now past [`MAX_SELECT_OPTIONS`].
    ///
    /// # Safety
    /// As [`current_node`](Self::current_node).
    unsafe fn count_option(&mut self) -> bool {
        let Some(raw) = self.current_node() else {
            return false;
        };
        // SAFETY: an open element of the tree being built, live for the parse,
        // and only read here (its tag, namespace and ancestors).
        let option = raw.as_node();
        if option.tag_id() != Some(TagId::OPTION) || option.ns_id() != Some(NsId::HTML) {
            return false; /* not inserted as an element that updates a select */
        }
        let Some(select) = nearest_select(option) else {
            return false;
        };
        let key = RawNode::from(select).as_ptr() as *const c_void;
        if key == self.select {
            self.options += 1;
        } else {
            self.select = key;
            self.options = 1;
        }
        self.options > MAX_SELECT_OPTIONS
    }
}

/// The `<select>` an inserted `<option>` updates, if any. Lexbor's static
/// `lxb_html_option_element_nearest_ancestor_select`, restated; keep it that
/// rule.
fn nearest_select(option: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut optgroup = false;
    let mut node = option.parent();
    while let Some(n) = node {
        if n.ns_id() == Some(NsId::HTML) {
            match n.tag_id() {
                Some(TagId::DATALIST | TagId::HR | TagId::OPTION) => return None,
                Some(TagId::OPTGROUP) if optgroup => return None,
                Some(TagId::OPTGROUP) => optgroup = true,
                Some(TagId::SELECT) => return Some(n),
                _ => {}
            }
        }
        node = n.parent();
    }
    None
}

/// The chained token-done callback.
///
/// Always delegates first, so the parser still builds the tree; then refuses
/// the token - which stops the parse - if the tree has grown past the limit.
/// A recording failure only stops the recording.
///
/// # Safety
/// Called by Lexbor's tokenizer only, with the context [`TokenHook::install`]
/// registered: `ctx` is that live, unmoved hook, `tkz` and `token` the
/// tokenizer's own, and the tree `install` found is alive for the parse.
unsafe extern "C" fn hook_token_cb(
    tkz: *mut Tokenizer,
    token: *mut Token,
    ctx: *mut c_void,
) -> *mut Token {
    let hook = &mut *(ctx as *mut TokenHook);
    if let Some(rec) = hook.recorder.as_mut() {
        if rec.recording() && !hook.panic.caught() {
            /* Catch rather than unwind into the tokenizer: this is called from
             * C. Recording then stops, the parse carries on, and the caller
             * raises the panic once C has unwound. */
            hook.panic.guard((), || rec.record(token));
        }
    }
    /* Read before delegating: the tree builder may reuse the token. Only an
     * `<option>` start tag can insert an option. */
    let option_start = (*token).tag_id == lxb::lxb_tag_id_enum_t_LXB_TAG_OPTION as usize
        && ((*token).type_ & lxb::lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE as i32) == 0;
    let out = match hook.delegate {
        Some(f) => f(tkz, token, hook.delegate_ctx),
        /* Unreachable in practice - `install` sets the delegate before the
         * first token - but returning the token unchanged is the one answer
         * that does not lose it. */
        None => token,
    };
    /* `out` is NULL when the tree builder itself failed; that stands. */
    if out.is_null() {
        return out;
    }
    if hook.max_open != usize::MAX && hook.open_elements() > hook.max_open {
        hook.stopped = Some(GuardStop::TooDeep);
        return core::ptr::null_mut(); /* the tokenizer stops with an error */
    }
    if option_start && hook.count_option() {
        hook.stopped = Some(GuardStop::TooManyOptions);
        return core::ptr::null_mut();
    }
    out
}

/// The document a fragment parse is building in, from the parser's tree.
///
/// `lxb_html_parse_fragment_chunk_begin` makes that document and attaches it to
/// the tree, and - when it was given no owner document - nothing in Lexbor
/// frees it: on success the fragment root still lives in it, and on a failed
/// `process` it is simply abandoned. Read right after `begin`, so the caller
/// can own it on every path.
///
/// # Safety
/// `parser` must be live, and `begin` must have succeeded on it.
pub(in crate::lexbor) unsafe fn fragment_document(
    parser: *mut lxb::lxb_html_parser_t,
) -> *mut lxb::lxb_html_document_t {
    let tree = lxb::lxb_html_parser_tree_noi(parser);
    if tree.is_null() {
        core::ptr::null_mut()
    } else {
        (*tree).document
    }
}
