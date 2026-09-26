//! Source-location tracking.
//!
//! Lexbor does not record where in the input a node came from, and we stay on
//! vanilla Lexbor, so it is reconstructed from the tokenizer instead:
//!
//! 1. [`Recorder`] rides the tokenizer's token-done hook
//!    (`tree_guard::TokenHook`, which also carries the tree-depth guard) and
//!    logs `(tag_id, byte offset)` for every element start-tag, in token order.
//! 2. After the tree is built, [`pos_assign_to_dom`] walks the DOM
//!    pre-order and matches each element to the next compatible recorded token,
//!    stamping the byte offset on it.
//! 3. [`lines_build`] maps a byte offset to a 1-based line for `Node#line`.
//!
//! Precision is about the HTML5 tree-construction reorderings (foster
//! parenting, the adoption agency) away from perfect. On a mismatch the node is
//! left UNSTAMPED - `#line` answers nil - rather than given a wrong location.
//!
//! # `node.user` is reserved
//!
//! CLAUDE.md reserves Lexbor's `node.user` for exactly this offset. The field and
//! its encoding belong to `html` (`HtmlNode::stamp_source_offset` /
//! `source_offset`); this module decides WHICH offset an element gets.
//!
//! # The recording runs inside Lexbor
//!
//! [`Recorder::record`] is called from the tokenizer's hook, from C, once per
//! token. It must not unwind and must not stop the parse. Both are structural:
//! the hook calls it under a panic latch, and it has no failure path of its
//! own - a recording failure only sets the overflow flag.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::falloc::{try_vec_with_capacity, VecPush};

use crate::lexbor::abi as lxb;

extern "C" {
    /// libc `memchr` - see [`next_newline`].
    #[link_name = "memchr"]
    fn libc_memchr(s: *const c_void, c: core::ffi::c_int, n: usize) -> *const c_void;
}

type Token = lxb::lxb_html_token_t;

use super::html::{HtmlNode, TagId, TAG_EM_DOCTYPE};
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
/// libc `memchr`.
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

/// A SOFT cap so a pathological input cannot make the transient recorder grow
/// without bound. Checked where the array would GROW (see `record`), so the
/// vec can already hold up to about one geometric step past this - the cap
/// bounds the order, it is not an exact length. On overflow recording stops
/// AND assignment is skipped entirely, so locations degrade to "unknown"
/// rather than to wrong values.
const MAX_TOKENS: usize = 10_000_000;

/// How far ahead of the cursor a match is looked for. This bounds the damage
/// from a dropped or reordered token: an unmatched element stays unstamped
/// instead of taking a far-away token's offset.
const LOOKAHEAD: usize = 64;

#[derive(Clone, Copy)]
struct Entry {
    tag_id: TagId,
    offset: usize,
}

/// The start-tag offsets a parse records, fed by the tokenizer's hook.
///
/// A plain value: it allocates nothing until the first token is recorded, and
/// then only through `falloc`, a failure of which stops the recording.
pub struct Recorder {
    items: Vec<Entry>,
    /// The start of the input buffer, which offsets are relative to.
    first: *const u8,
    overflow: bool,
}

/// The recorded offsets, detached from the parse that produced them.
///
/// Only this survives the parse: `Recorder` also holds a pointer INTO the
/// source buffer, which is freed when the parse returns. Carrying that into
/// the document would be a dangling pointer nothing needs - the offsets were
/// resolved against `first` as each token arrived.
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

    /// A recorder for the tokens of the input that starts at `src`.
    pub fn new(src: *const u8) -> Recorder {
        Recorder {
            items: Vec::new(),
            first: src,
            overflow: false,
        }
    }

    /// Whether tokens are still being recorded - false once the cap or an
    /// allocation failure stopped it.
    #[inline]
    pub fn recording(&self) -> bool {
        !self.overflow
    }

    /// Record one token, if it is an element start-tag.
    ///
    /// # Safety
    /// `token` must be the live token the tokenizer is handing out, and it
    /// must point into the input that starts at `first`.
    #[inline]
    pub unsafe fn record(&mut self, token: *const Token) {
        record(self, token);
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
    /* Above the special ids, so never UNDEF. */
    let Some(tag_id) = TagId::from_raw((*token).tag_id) else {
        return;
    };

    /* The cap is checked where the array would GROW, so it stops the next
     * growth past MAX_TOKENS; tokens that fit the capacity already allocated
     * are still recorded. (Checking on every push would record fewer.) */
    let at_growth = rec.items.len() == rec.items.capacity();
    if at_growth && rec.items.len() >= MAX_TOKENS {
        rec.overflow = true; /* fail closed: stop recording */
        return;
    }
    let entry = Entry {
        tag_id,
        offset: (*token).begin as usize - rec.first as usize,
    };
    if rec.items.falloc_push(entry).is_err() {
        rec.overflow = true;
    }
}

/* ------------------------------------------------------------------ *
 * assignment                                                         *
 * ------------------------------------------------------------------ */

/// Stamp each element's recorded byte offset (`HtmlNode::stamp_source_offset`).
///
/// Walks the DOM in document order alongside the recorded tokens, matching by
/// tag id within a bounded lookahead. An element with no match in that window is
/// left unstamped; `#line` then answers nil, which is the whole point - never a
/// wrong line.
pub fn pos_assign_to_dom(rec: &Positions, root: HtmlNode<'_>) {
    if rec.overflow {
        return;
    }

    let mut cursor = 0usize;
    for el in root.subtree().filter_map(HtmlNode::element) {
        if cursor >= rec.items.len() {
            break;
        }
        let Some(tid) = el.node().tag_id() else {
            continue; /* no token has an UNDEF id to match */
        };
        let limit = (cursor + LOOKAHEAD).min(rec.items.len());
        if let Some(j) = (cursor..limit).find(|&j| rec.items[j].tag_id == tid) {
            el.node().stamp_source_offset(rec.items[j].offset);
            cursor = j + 1;
        }
    }
}
