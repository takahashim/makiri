//! The tree-depth guard every HTML parse runs under, and the tokenizer hook
//! that carries it.
//!
//! # Why a limit
//!
//! HTML tree construction is quadratic in nesting depth: nearly every start
//! tag asks a scope question ("has a `p` element in button scope") by walking
//! the stack of open elements, so `"<div>" * 80_000` - 400 KB - took five
//! seconds, and every doubling quadruples it. The parse runs with the GVL
//! released and nothing can interrupt it, which makes that a denial of service
//! for a server parsing untrusted HTML. Browsers and Gumbo bound the depth for
//! the same reason (Chrome at 512, Nokogiri::HTML5's `max_tree_depth` at 400),
//! and Makiri's own XML reader has done so from the start (`xml::MAX_DEPTH`).
//!
//! # What is measured
//!
//! The DEPTH of an element is the number of elements from the root down to it,
//! itself included: in a document `<html>` is 1, `<body>` 2, and the Nth
//! nested `<div>` inside it N + 2. In a fragment the top-level elements are 1.
//! That is Nokogiri::HTML5's count, checked against it at the boundary: with
//! the default 400, a document holds 398 nested `<div>`s in its body and a
//! fragment 400.
//!
//! What is read is the tree builder's stack of open elements, after the tree
//! builder has processed each token. In a document that stack IS the ancestor
//! chain of the insertion point - `html`, then `body`, then the rest. A
//! fragment parse keeps one synthetic `<html>` root at the bottom of it, which
//! is not part of the fragment, so a fragment's allowance is one entry longer
//! ([`TokenHook::new`]'s `synthetic`). A parse stops as soon as the stack
//! holds more than the limit allows.
//!
//! Checking per TOKEN rather than per push is enough: one token can push
//! several elements (the implied `html`/`body`, or the reconstruction of the
//! active formatting elements), but every one of those is bounded by what the
//! stack held after the previous token, which the guard had already accepted.
//!
//! # Stopping the parse
//!
//! The hook returns NULL for the token, which is how Lexbor's own tree builder
//! reports a failure: the tokenizer sets `LXB_STATUS_ERROR` and stops, and the
//! parse call returns that status. [`TokenHook::too_deep`] is what tells the
//! caller the failure was the limit rather than something else.
//!
//! # Fail-closed, whatever the source recorder does
//!
//! The hook is a plain value on the caller's stack, installed on EVERY parse;
//! nothing about it allocates. The document parse's source-position
//! [`Recorder`] rides inside it, and its failure modes - an allocation that
//! fails, the token cap, a caught panic - only stop the RECORDING. The depth
//! check does not depend on the recorder at all, so it runs regardless.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::caught::PanicLatch;
use crate::lexbor::abi as lxb;

use super::source_loc::Recorder;

type Token = lxb::lxb_html_token_t;
type Tokenizer = lxb::lxb_html_tokenizer_t;
type TokenFn = lxb::lxb_html_tokenizer_token_f;
type Tree = lxb::lxb_html_tree_t;

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
    too_deep: bool,
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
            too_deep: false,
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

    /// Whether the parse was stopped by the depth limit.
    pub fn too_deep(&self) -> bool {
        self.too_deep
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
    let out = match hook.delegate {
        Some(f) => f(tkz, token, hook.delegate_ctx),
        /* Unreachable in practice - `install` sets the delegate before the
         * first token - but returning the token unchanged is the one answer
         * that does not lose it. */
        None => token,
    };
    /* `out` is NULL when the tree builder itself failed; that stands. */
    if !out.is_null() && hook.max_open != usize::MAX && hook.open_elements() > hook.max_open {
        hook.too_deep = true;
        return core::ptr::null_mut(); /* the tokenizer stops with an error */
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
