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

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::falloc::{try_box_raw, try_vec_with_capacity, Reserve};

use crate::lexbor_abi::{self as lxb, preorder_next, LxbNode};

extern "C" {
    /// libc `memchr` - see [`next_newline`].
    #[link_name = "memchr"]
    fn libc_memchr(s: *const c_void, c: core::ffi::c_int, n: usize) -> *const c_void;
}

pub type Parsed = lxb::mkr::mkr_parsed_t;
type Token = lxb::lxb_html_token_t;
type Tokenizer = lxb::lxb_html_tokenizer_t;
type TokenFn = lxb::lxb_html_tokenizer_token_f;

const NODE_TYPE_ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
/// The last of Lexbor's special tag ids (text, comment, doctype, document, eof).
/// A token at or below it is not an element start-tag.
const TAG_EM_DOCTYPE: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__EM_DOCTYPE as usize;
const TOKEN_TYPE_CLOSE: i32 = lxb::lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE as i32;

/* ------------------------------------------------------------------ *
 * line table                                                         *
 * ------------------------------------------------------------------ */

struct Lines {
    /// `starts[i]` is the byte offset of line `i + 1`; `starts[0] == 0`.
    starts: Vec<usize>,
}

impl Lines {
    /// The largest line whose start offset is `<= offset`, 1-based.
    ///
    /// `partition_point` is the same binary search the C wrote out: the count of
    /// starts at or below the offset IS the 1-based line.
    fn lookup(&self, offset: usize) -> usize {
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
unsafe fn next_newline(bytes: &[u8], from: usize) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    let p = libc_memchr(
        bytes.as_ptr().add(from) as *const c_void,
        b'\n' as core::ffi::c_int,
        bytes.len() - from,
    );
    if p.is_null() {
        None
    } else {
        Some(p as usize - bytes.as_ptr() as usize)
    }
}

/// Build the line table over the input. NULL on allocation failure, which the
/// caller treats as "no line information" rather than as a parse failure.
pub unsafe fn lines_build(src: *const u8, len: usize) -> *mut c_void {
    let bytes: &[u8] = if src.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src, len)
    };

    /* Count first, so the array is sized exactly once. Both passes go through
     * `next_newline`, so they cannot disagree about which bytes start a line. */
    let mut nl = 0usize;
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        nl += 1;
        at = i + 1;
    }

    let mut starts: Vec<usize> = match try_vec_with_capacity(nl + 1) {
        Some(v) => v,
        None => return core::ptr::null_mut(),
    };

    starts.push(0); /* reserved above; cannot allocate */
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        starts.push(i + 1); /* the next line starts past the newline */
        at = i + 1;
    }

    match try_box_raw(Lines { starts }) {
        p if p.is_null() => core::ptr::null_mut(),
        p => p as *mut c_void,
    }
}

pub unsafe fn lines_free(lines: *mut c_void) {
    if !lines.is_null() {
        drop(Box::from_raw(lines as *mut Lines));
    }
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

    /// The parser's OWN token-done callback, which actually builds the tree.
    orig: TokenFn,
    orig_ctx: *mut c_void,
}

pub unsafe fn pos_recorder_create(src: *const u8) -> *mut Recorder {
    try_box_raw(Recorder {
        items: Vec::new(),
        first: src,
        overflow: false,
        orig: None,
        orig_ctx: core::ptr::null_mut(),
    })
}

pub unsafe fn pos_recorder_destroy(rec: *mut Recorder) {
    if !rec.is_null() {
        drop(Box::from_raw(rec));
    }
}

pub unsafe fn pos_recorder_set_delegate(rec: *mut Recorder, orig: TokenFn, orig_ctx: *mut c_void) {
    if rec.is_null() {
        return;
    }
    (*rec).orig = orig;
    (*rec).orig_ctx = orig_ctx;
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
    if !rec.overflow {
        record(rec, token);
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
pub unsafe fn pos_assign_to_dom(rec: *mut Recorder, root: *mut LxbNode) {
    if rec.is_null() || (*rec).overflow || root.is_null() {
        return;
    }
    let rec = &*rec;

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

/* ------------------------------------------------------------------ *
 * line lookup for Node#line                                          *
 * ------------------------------------------------------------------ */

/// The 1-based source line for `node`, or 0 when unknown.
///
/// 0 covers both "the tracker could not place this node" and "the line table
/// could not be allocated". The two are deliberately not distinguished: the
/// Ruby contract for `#line` is an Integer or nil, and post_parse documents the
/// table's allocation as an allowed degradation - see the note in the
/// html_node_read OOM scenario.
pub unsafe fn parsed_node_line(p: *mut Parsed, node: *const LxbNode) -> usize {
    if p.is_null() || node.is_null() || (*node).user.is_null() || (*p).newline_idx.is_null() {
        return 0;
    }
    let offset = (*node).user as usize - 1;
    (*((*p).newline_idx as *const Lines)).lookup(offset)
}
