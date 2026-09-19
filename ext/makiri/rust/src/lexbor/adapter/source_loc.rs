//! Source-location tracking (dom_adapter/source_loc.c).
//!
//! Lexbor does not record where in the input a node came from, and we stay on
//! vanilla Lexbor, so it is reconstructed from the tokenizer instead:
//!
//! 1. [`Recorder`] CHAINS the tokenizer's token-done callback and logs
//!    `(tag_id, byte offset)` for every element start-tag, in token order.
//! 2. After the tree is built, [`pos_assign_to_dom`] walks the DOM
//!    pre-order and matches each element to the next compatible recorded token,
//!    stamping the byte offset into `node.user`.
//! 3. [`lines_build`] maps a byte offset to a 1-based line for `Node#line`.
//!
//! Precision is about the HTML5 tree-construction reorderings (foster
//! parenting, the adoption agency) away from perfect. On a mismatch the node is
//! left UNSTAMPED - `#line` answers nil - rather than given a wrong location.
//!
//! # `node.user` is reserved
//!
//! CLAUDE.md reserves Lexbor's `node.user` for exactly this offset. Nothing else
//! in the extension may write it.
//!
//! # The callback runs inside Lexbor
//!
//! [`pos_token_cb`] is called by the tokenizer, from C, once per token. It
//! must not unwind and must always delegate, or the parser stops building the
//! tree. Both are structural here: it has no failure path of its own - a
//! recording failure only sets the overflow flag - and the delegation is the
//! tail of the function.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::falloc::{try_vec_with_capacity, Reserve};

use crate::lexbor_abi::{self as lxb, preorder_next, LxbNode};

extern "C" {
    /// libc `memchr` - see [`next_newline`].
    #[link_name = "memchr"]
    fn libc_memchr(s: *const c_void, c: core::ffi::c_int, n: usize) -> *const c_void;
}

type Token = lxb::lxb_html_token_t;
type Tokenizer = lxb::lxb_html_tokenizer_t;
type TokenFn = lxb::lxb_html_tokenizer_token_f;

use super::html::{TAG_EM_DOCTYPE, TYPE_ELEMENT as NODE_TYPE_ELEMENT};
const TOKEN_TYPE_CLOSE: i32 = lxb::lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE as i32;

/* ------------------------------------------------------------------ *
 * line table                                                         *
 * ------------------------------------------------------------------ */

pub struct Lines {
    /// `starts[i]` is the byte offset of line `i + 1`; `starts[0] == 0`.
    starts: Vec<usize>,
}

impl Lines {
    /// The largest line whose start offset is `<= offset`, 1-based.
    ///
    /// `partition_point` is the same binary search the C wrote out: the count of
    /// starts at or below the offset IS the 1-based line.
    pub fn lookup(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }
}

/// The offset of the next newline at or after `from`, or `None`.
///
/// libc `memchr`, which is what the C reached for through `mkr_span_find`.
/// A scalar `iter().position()` here measured about 9% off the whole parse on a
/// 220 KB document - the line table walks every input byte twice, so the
/// difference between a vectorised scan and a byte loop is the difference
/// between this subsystem being free and being visible.
#[inline]
fn next_newline(bytes: &[u8], from: usize) -> Option<usize> {
    let rest = bytes.get(from..).filter(|r| !r.is_empty())?;
    // SAFETY: `rest` is `rest.len()` readable bytes.
    let p = unsafe {
        libc_memchr(
            rest.as_ptr() as *const c_void,
            b'\n' as core::ffi::c_int,
            rest.len(),
        )
    };
    if p.is_null() {
        None
    } else {
        Some(p as usize - bytes.as_ptr() as usize)
    }
}

/// Build the line table over the input. None on allocation failure, which the
/// caller treats as "no line information" rather than as a parse failure.
pub fn lines_build(bytes: &[u8]) -> Option<Lines> {
    /* Count first, so the array is sized exactly once. Both passes go through
     * `next_newline`, so they cannot disagree about which bytes start a line. */
    let mut nl = 0usize;
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        nl += 1;
        at = i + 1;
    }

    let mut starts: Vec<usize> = try_vec_with_capacity(nl + 1)?;

    starts.push(0); /* reserved above; cannot allocate */
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        starts.push(i + 1); /* the next line starts past the newline */
        at = i + 1;
    }

    Some(Lines { starts })
}

/* ------------------------------------------------------------------ *
 * token position recorder                                            *
 * ------------------------------------------------------------------ */

/// A defensive cap so a pathological input cannot make the transient recorder
/// grow without bound. On overflow recording stops AND assignment is skipped
/// entirely, so locations degrade to "unknown" rather than to wrong values.
const MAX_TOKENS: usize = 10_000_000;

/// How far ahead of the cursor a match is looked for. This bounds the damage
/// from a dropped or reordered token: an unmatched element stays unstamped
/// instead of taking a far-away token's offset.
const LOOKAHEAD: usize = 64;

#[derive(Clone, Copy)]
struct Entry {
    tag_id: usize,
    offset: usize,
}

/// `mkr_pos_recorder_t`, opaque to C.
pub struct Recorder {
    items: Vec<Entry>,
    /// The start of the input buffer, which offsets are relative to.
    first: *const u8,
    overflow: bool,
    /// A panic in `record`, latched rather than raised: this runs from
    /// Lexbor's tokenizer, and unwinding into C aborts. `parse_tracked` raises
    /// it after the parse has unwound (see `crate::caught`).
    panic: crate::caught::PanicLatch,

    /// The parser's OWN token-done callback, which actually builds the tree.
    orig: TokenFn,
    orig_ctx: *mut c_void,
}

/// The recorded offsets, detached from the parse that produced them.
///
/// Only this survives the parse: `Recorder` also holds a pointer INTO the
/// source buffer, which is freed when the parse returns, and the tokenizer
/// delegate, which is gone with the parser. Carrying those into the document
/// would be a dangling pointer nothing needs - the offsets were resolved
/// against `first` as each token arrived.
pub struct Positions {
    items: Vec<Entry>,
    overflow: bool,
}

impl Recorder {
    /// What the document keeps: the offsets, without the parse-time pointers.
    pub fn into_positions(self) -> Positions {
        Positions {
            items: self.items,
            overflow: self.overflow,
        }
    }

    /// Re-raise a panic the token callback caught, now that Lexbor's frames
    /// are gone. A no-op when nothing panicked.
    pub fn resume_panic(&mut self) {
        self.panic.resume();
    }

    /// A recorder for the tokens of the input that starts at `src`.
    pub fn new(src: *const u8) -> Recorder {
        Recorder {
            items: Vec::new(),
            first: src,
            overflow: false,
            panic: crate::caught::PanicLatch::new(),
            orig: None,
            orig_ctx: core::ptr::null_mut(),
        }
    }

    /// The parser's own token-done callback, which every token is passed on to.
    pub fn set_delegate(&mut self, orig: TokenFn, orig_ctx: *mut c_void) {
        self.orig = orig;
        self.orig_ctx = orig_ctx;
    }
}

/// Record one token, if it is an element start-tag.
///
/// The special tag ids (text, comment, doctype, document, eof) sit at or below
/// `LXB_TAG__EM_DOCTYPE` and are skipped, as are end-tags. `CLOSE_SELF` - a
/// void or self-closing start tag such as `<br/>` - leaves the CLOSE bit clear,
/// so it IS recorded.
unsafe fn record(rec: &mut Recorder, token: *const Token) {
    if (*token).tag_id <= TAG_EM_DOCTYPE
        || (*token).begin.is_null()
        || ((*token).type_ & TOKEN_TYPE_CLOSE) != 0
    {
        return;
    }

    if rec.items.len() == rec.items.capacity() {
        /* Fail closed BEFORE the geometric growth can exceed the cap. */
        if rec.items.len() >= MAX_TOKENS {
            rec.overflow = true;
            return;
        }
        let want = match crate::falloc::grow_capacity(
            rec.items.capacity(),
            rec.items.len() + 1,
            core::mem::size_of::<Entry>(),
        ) {
            Some(w) => w,
            None => {
                rec.overflow = true;
                return;
            }
        };
        if rec.items.mkr_reserve_exact(want - rec.items.len()).is_err() {
            rec.overflow = true; /* fail closed: stop recording */
            return;
        }
    }

    rec.items.push(Entry {
        tag_id: (*token).tag_id,
        offset: (*token).begin as usize - rec.first as usize,
    });
}

/// The chained token-done callback, installed on the tokenizer.
///
/// Always delegates, so the parser still builds the tree; a recording failure
/// only sets the overflow flag, which later suppresses assignment.
pub unsafe extern "C" fn pos_token_cb(
    tkz: *mut Tokenizer,
    token: *mut Token,
    ctx: *mut c_void,
) -> *mut Token {
    let rec = &mut *(ctx as *mut Recorder);
    if !rec.overflow && !rec.panic.caught() {
        /* Catch rather than unwind into the tokenizer: this is called from C.
         * The latch is moved out and back so `record` can take `rec` mutably;
         * recording then stops, the parse carries on through the delegate
         * below, and `parse_tracked` raises the panic once C has unwound. */
        let mut latch = core::mem::take(&mut rec.panic);
        latch.guard((), || record(rec, token));
        rec.panic = latch;
    }
    match rec.orig {
        Some(f) => f(tkz, token, rec.orig_ctx),
        /* Unreachable in practice - post_parse installs the delegate before the
         * first token - but returning the token unchanged is the one answer that
         * does not lose it. */
        None => token,
    }
}

/* ------------------------------------------------------------------ *
 * assignment                                                         *
 * ------------------------------------------------------------------ */

/// Stamp each element's recorded byte offset into `node.user`.
///
/// Walks the DOM in document order alongside the recorded tokens, matching by
/// tag id within a bounded lookahead. An element with no match in that window is
/// left unstamped; `#line` then answers nil, which is the whole point - never a
/// wrong line.
pub unsafe fn pos_assign_to_dom(rec: &Positions, root: *mut LxbNode) {
    if rec.overflow || root.is_null() {
        return;
    }

    let mut cursor = 0usize;
    let mut node = root;
    while !node.is_null() {
        if (*node).type_ == NODE_TYPE_ELEMENT {
            if cursor >= rec.items.len() {
                break;
            }
            let tid = (*node).local_name;
            let limit = (cursor + LOOKAHEAD).min(rec.items.len());
            for j in cursor..limit {
                if rec.items[j].tag_id == tid {
                    /* +1 so a genuine offset of 0 is distinguishable from unset. */
                    (*node).user = (rec.items[j].offset + 1) as *mut c_void;
                    cursor = j + 1;
                    break;
                }
            }
        }
        node = preorder_next(node, root);
    }
}
