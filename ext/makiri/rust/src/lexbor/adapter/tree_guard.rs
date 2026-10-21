//! The guard every HTML parse runs under: a tokenizer hook that stops the
//! parse on the input shape Lexbor's tree construction handles in quadratic
//! time. The parse runs with the GVL released and cannot be
//! interrupted, so it is a denial of service on untrusted input.
//!
//! - **Depth.** Most start tags walk the stack of open elements for a scope
//!   check, so `"<div>" * 80_000` (400 KB) took 5 s. The limit is Nokogiri's
//!   `max_tree_depth` (default 400) with its boundary: an element's depth
//!   counts itself and its ancestors, `<html>` being 1 in a document and the
//!   top level 1 in a fragment (whose parser keeps a synthetic `<html>` below
//!   it - the `synthetic` of [`TokenHook::for_document`] / [`TokenHook::for_fragment`]). Checked on the open-element stack
//!   after each token; a token that pushes several elements is bounded by what
//!   the previous check accepted.
//! - **Options per select** were limited here too (10,000), for Lexbor's
//!   selectedness algorithm, which walks every option per option inserted.
//!   The parser port no longer walks so, and has no limit; the reference
//!   keeps the same verdicts by having none either.
//!
//! The hook stops a parse by returning NULL for the token - how Lexbor's own
//! tree builder fails - and records why ([`TokenHook::stopped`]).
//!
//! It is a plain value on the caller's stack, installed on every parse, and
//! allocates nothing. The source-position [`Stamper`] rides inside it. A panic
//! anywhere in the hook is latched and stops the parse; the caller raises it
//! once Lexbor has returned.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::caught::PanicLatch;
use crate::lexbor::abi as lxb;

use super::html::RawNode;
use super::source_loc::{stamp_created, Before, Stamper};

type Token = lxb::lxb_html_token_t;
type Tokenizer = lxb::lxb_html_tokenizer_t;
type TokenFn = lxb::lxb_html_tokenizer_token_f;
type Tree = lxb::lxb_html_tree_t;

/* The limits and stop reasons are the ported tree builder's, so both
 * parsers answer with the same types. */
pub use crate::html::tree::guard::{DepthLimit, GuardStop};

/// The tokenizer's token-done hook: runs the depth guard after the tree
/// builder, and stamps source positions for a document parse.
///
/// Installed with its own address as the callback context, so it must stay
/// where it is, and alive, from [`install`](Self::install) until the parse
/// call has returned.
///
/// `PANIC_PROBE` exists for `Makiri.__panic(6)` alone: a hook built with it
/// panics inside the guarded part of every token, which is how the suite proves
/// the latch covers that part. It is a const parameter so an ordinary parse
/// (`false`, the default) compiles the probe out entirely.
pub struct TokenHook<const PANIC_PROBE: bool = false> {
    state: HookState,
    /// A panic anywhere in the hook's Rust, latched rather than raised: this
    /// runs from Lexbor's tokenizer, and unwinding into C aborts. The token
    /// that panicked stops the parse, and the caller raises the panic once the
    /// parse has returned (see `crate::caught`).
    panic: PanicLatch,
}

/// Everything the hook keeps across tokens except the latch, so the callback
/// can lend all of it to the one guarded closure (see [`hook_token_cb`]).
struct HookState {
    /// The parser's OWN token-done callback, which builds the tree.
    delegate: TokenFn,
    delegate_ctx: *mut c_void,
    /// The tree builder whose stack of open elements is measured.
    tree: *const Tree,
    /// The stack length above which the parse stops; `usize::MAX` for none.
    max_open: usize,
    stopped: Option<GuardStop>,
    /// The document parse's position stamper; `None` for a fragment.
    stamper: Option<Stamper>,
}

impl TokenHook {
    /// The hook a fragment parse installs: `limit` over the fragment's own
    /// elements (the parser keeps one synthetic `<html>` root below the
    /// first). No position stamper: a fragment's elements have no line.
    pub fn for_fragment(limit: DepthLimit) -> TokenHook {
        TokenHook::build(limit, 1, None)
    }
}

impl<const PANIC_PROBE: bool> TokenHook<PANIC_PROBE> {
    /// The hook a document parse installs: `limit` from the root (nothing
    /// synthetic below it), and `stamper` recording each element's position.
    pub fn for_document(limit: DepthLimit, stamper: Stamper) -> Self {
        TokenHook::build(limit, 0, Some(stamper))
    }

    /// A hook enforcing `limit`, where the parser keeps `synthetic` entries on
    /// its stack below the first real element (0 for a document, 1 for a
    /// fragment's `<html>` root).
    fn build(limit: DepthLimit, synthetic: usize, stamper: Option<Stamper>) -> Self {
        TokenHook {
            state: HookState {
                delegate: None,
                delegate_ctx: core::ptr::null_mut(),
                tree: core::ptr::null(),
                max_open: limit.max_open(synthetic).unwrap_or(usize::MAX),
                stopped: None,
                stamper,
            },
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
        self.state.tree = tree;
        let tkz = lxb::lxb_html_parser_tokenizer_noi(parser);
        /* Lexbor has a setter and a ctx getter for the token-done callback but
         * no getter for the callback FUNCTION, so that one field is read from
         * the struct directly; the ctx uses the public accessor. */
        self.state.delegate = (*tkz).callback_token_done;
        self.state.delegate_ctx = lxb::lxb_html_tokenizer_callback_token_done_ctx_noi(tkz);
        lxb::lxb_html_tokenizer_callback_token_done_set_noi(
            tkz,
            Some(hook_token_cb::<PANIC_PROBE>),
            self as *mut Self as *mut c_void,
        );
        true
    }

    /// The guard's verdict on a parse that has returned: re-raise a panic the
    /// hook caught - now that Lexbor's frames are gone, the first frame where
    /// that is safe - and then `Err` with what stopped the parse, if the hook
    /// did. Before the caller's own status check: a panic is not a parse
    /// failure, and neither is a refusal - the hook stops the parse by failing
    /// its status.
    pub fn finish(&mut self) -> Result<(), GuardStop> {
        self.panic.resume();
        match self.state.stopped {
            Some(stop) => Err(stop),
            None => Ok(()),
        }
    }
}

impl HookState {
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

    /// One token: everything the hook does with it, the delegate included.
    ///
    /// Always delegates, so the parser still builds the tree; then refuses the
    /// token - which stops the parse - if the tree has grown past the limit. For a document parse, an element start tag's
    /// offset is stamped onto the element the tree builder created for it
    /// (`source_loc::stamp_created`), from the current node seen on either
    /// side of the delegate.
    ///
    /// # Safety
    /// As [`hook_token_cb`].
    #[inline]
    #[allow(
        clippy::panic,
        reason = "the `Makiri.__panic(6)` probe, compiled out unless PANIC_PROBE"
    )]
    unsafe fn on_token<const PANIC_PROBE: bool>(
        &mut self,
        tkz: *mut Tokenizer,
        token: *mut Token,
    ) -> *mut Token {
        /* Read before delegating: the tree builder may reuse the token. */
        let start = match self.stamper.as_ref() {
            Some(st) => st.start_tag(token),
            None => None,
        };
        // SAFETY: the stack's entries are the live elements of the tree being
        // built, which the parse does not free; each handle is used only
        // within this call, before the parse can go on.
        let before = start.map(|_| Before::at(self.current_node().map(|r| r.as_node())));
        let out = match self.delegate {
            /* Lexbor's tree builder: C that calls nothing of ours, so nothing
             * can unwind through it from here. */
            Some(f) => f(tkz, token, self.delegate_ctx),
            /* Unreachable in practice - `install` sets the delegate before the
             * first token - but returning the token unchanged is the one
             * answer that does not lose it. */
            None => token,
        };
        /* `out` is NULL when the tree builder itself failed; that stands. */
        if out.is_null() {
            return out;
        }
        if PANIC_PROBE {
            panic!("Makiri.__panic(6): panic inside the tokenizer hook");
        }
        if let (Some(tag), Some(before)) = (start, before) {
            // SAFETY: as `before`.
            let now = self.current_node().map(|r| r.as_node());
            stamp_created(tag, before, now);
        }
        if self.max_open != usize::MAX && self.open_elements() > self.max_open {
            self.stopped = Some(GuardStop::TooDeep);
            return core::ptr::null_mut(); /* the tokenizer stops with an error */
        }
        out
    }
}

/// The chained token-done callback: [`HookState::on_token`], under the latch.
///
/// ALL of the hook's Rust runs inside the one closure given to
/// `PanicLatch::guard`, so nothing added to `on_token` can land outside it and
/// unwind into the tokenizer - which, being C, would abort the host. The
/// delegate is called from inside the closure too: it is Lexbor's C and calls
/// nothing of ours, so a catch around it changes nothing about how the tree is
/// built. A caught panic returns NULL for the token, which stops the parse the
/// way a refused token does; the caller re-raises it once the parse has
/// returned ([`TokenHook::finish`]). A token after a panic - none is
/// expected, the tokenizer stops - is refused without running anything.
///
/// # Safety
/// Called by Lexbor's tokenizer only, with the context [`TokenHook::install`]
/// registered: `ctx` is that live, unmoved hook, `tkz` and `token` the
/// tokenizer's own, and the tree `install` found is alive for the parse.
unsafe extern "C" fn hook_token_cb<const PANIC_PROBE: bool>(
    tkz: *mut Tokenizer,
    token: *mut Token,
    ctx: *mut c_void,
) -> *mut Token {
    let hook = &mut *(ctx as *mut TokenHook<PANIC_PROBE>);
    if hook.panic.caught() {
        return core::ptr::null_mut();
    }
    let state = &mut hook.state;
    hook.panic.guard(core::ptr::null_mut(), || {
        // SAFETY: this function's own contract, passed on unchanged.
        unsafe { state.on_token::<PANIC_PROBE>(tkz, token) }
    })
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
