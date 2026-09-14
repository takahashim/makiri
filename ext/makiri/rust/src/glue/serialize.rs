//! HTML serialization (glue/ruby_html_serialize.c).
//!
//!   `Node#to_html` / `#to_s` / `#outer_html` -> the node and its subtree
//!   `Node#inner_html`                        -> the node's children only
//!
//! Lexbor streams the output in many small chunks (one per tag, attribute or
//! text piece). They are collected into a single growing C buffer and copied
//! into a Ruby String once at the end, rather than appended to a Ruby String per
//! chunk - the per-chunk capacity check and coderange bookkeeping was the
//! serializer's dominant cost. The buffer is pre-reserved to roughly the output
//! size so those appends do not realloc on each geometric step. Lexbor emits
//! UTF-8, which is the string's encoding.

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;
use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};

use super::abi::*;
use crate::cbuf::{mkr_buf_append, Buf};

/// Lexbor's chunk sink. Must not panic: it is called from C.
unsafe extern "C" fn serialize_cb(data: *const u8, len: usize, ctx: *mut c_void) -> u32 {
    if mkr_buf_append(ctx as *mut Buf, data as *const c_void, len) == crate::cbuf::MKR_OK {
        LXB_STATUS_OK
    } else {
        LXB_STATUS_ERROR_MEMORY_ALLOCATION
    }
}

extern "C" {
    fn lxb_html_serialize_tree_cb(
        node: *mut LxbNode,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_deep_cb(
        node: *mut LxbNode,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_pretty_tree_cb(
        node: *mut LxbNode,
        opt: u32,
        indent: usize,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_pretty_deep_cb(
        node: *mut LxbNode,
        opt: u32,
        indent: usize,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
}

/// The buffer's ceiling and its initial reservation, both derived from the
/// document's live bytes in one walk.
///
/// The ceiling is the Lexbor analogue of the XML serializer's `arena_bytes` cap:
/// 32x the live bytes (covering escaping plus maximal pretty indentation) over a
/// 64 KiB floor - tight for a small document yet scaling with a large one, so a
/// legitimate parse round-trips through `to_html` (HTML parsing is itself
/// byte-uncapped) while a pathologically deep pretty-print fails closed instead
/// of growing without bound. It is deliberately *not* clamped to
/// `MKR_BUF_HARD_MAX` here; `mkr_buf.c` takes the minimum of the two on every
/// growth, so the clamp would only duplicate a constant this side could then
/// disagree with. The HTML tree cannot cycle (the mutation guards plus Lexbor's
/// own insert checks), so the ceiling is never reached in normal operation.
///
/// The reservation is ~live/4: serialized output is a fraction of the arena
/// (96-byte node structs dwarf their markup), which pre-sizes close to the real
/// output without a wasteful over-allocation and leaves geometric growth to
/// cover any underestimate.
fn serialize_sizes(live: usize) -> (usize, usize) {
    let cap = 65536usize.saturating_add(live.saturating_mul(32));
    let reserve = (live / 4).max(4096);
    (cap, reserve)
}

/// Serialize `node` into a fresh UTF-8 String. `deep` selects the children-only
/// (inner) serializer over the tree (outer) one; `pretty` selects indented
/// output.
fn serialize(ruby: &Ruby, node: *mut LxbNode, deep: bool, pretty: bool) -> Result<RString, Error> {
    let utf8 = ruby.utf8_encoding();
    // SAFETY: `node` came from a live wrapper, so its document is live too.
    let (cap, reserve) = serialize_sizes(unsafe { mkr_lxb_document_bytes(node) });

    let mut buf = Buf::new(cap);
    // SAFETY: `buf` is freed on both paths below, and nothing between here and
    // there can raise - the Err is returned, not thrown.
    unsafe {
        let _ = buf.reserve(reserve); /* best-effort pre-size */

        let ctx = &mut buf as *mut Buf as *mut c_void;
        let st = match (deep, pretty) {
            (true, true) => lxb_html_serialize_pretty_deep_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                serialize_cb,
                ctx,
            ),
            (true, false) => lxb_html_serialize_deep_cb(node, serialize_cb, ctx),
            (false, true) => lxb_html_serialize_pretty_tree_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                serialize_cb,
                ctx,
            ),
            (false, false) => lxb_html_serialize_tree_cb(node, serialize_cb, ctx),
        };

        if st != LXB_STATUS_OK {
            buf.free();
            return Err(Error::new(error_class(), "HTML serialization failed"));
        }
        // Lexbor emits UTF-8, so the String is tagged UTF-8 rather than built
        // as binary and re-tagged (which is what str_from_slice would give).
        let str = ruby.enc_str_new(buf.as_slice(), utf8);
        buf.free();
        Ok(str)
    }
}

/// The optional `pretty:` keyword.
///
/// Read for truthiness rather than converted to `bool`, which is what
/// `RTEST(rb_hash_aref(opts, :pretty))` did: `pretty: nil` is false and any
/// other value - `0` included, this being Ruby - is true. Unknown keywords are
/// ignored, as the C's `rb_scan_args(argc, argv, "0:", ...)` did, which is also
/// why the hash is read with a plain lookup rather than `get_kwargs`: that
/// allocates a second hash to hold the keys it was not asked about.
///
/// The no-argument call returns before any of that. It is the overwhelmingly
/// common one - `to_html` with a keyword is the exception - and routing it
/// through `scan_args` cost about a quarter of the per-call throughput on a
/// small element, which is all this method does at that size.
fn pretty_opt(ruby: &Ruby, args: &[Value]) -> Result<bool, Error> {
    if args.is_empty() {
        return Ok(false);
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    Ok(scanned
        .keywords
        .get(ruby.to_symbol("pretty"))
        .is_some_and(|v: Value| v.to_bool()))
}

/// Outer HTML: the node itself plus its descendants. `pretty: true` indents.
fn to_html(rb_self: Value, args: &[Value]) -> Result<RString, Error> {
    let ruby = Ruby::get_with(rb_self);
    let pretty = pretty_opt(&ruby, args)?;
    // The raising accessor, called while nothing is live (see the module docs).
    let node = unsafe { mkr_html_node_unwrap(rb_self.as_raw()) };

    // A document fragment has no tag of its own, so its "outer" is its
    // children: the deep serializer is the right one (the tree serializer
    // rejects a fragment node).
    let deep = unsafe { lxb_dom_node_type_noi(node) } == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
    serialize(&ruby, node, deep, pretty)
}

/// Inner HTML: the node's children, without the node's own tag.
fn inner_html(rb_self: Value, args: &[Value]) -> Result<RString, Error> {
    let ruby = Ruby::get_with(rb_self);
    let pretty = pretty_opt(&ruby, args)?;
    let node = unsafe { mkr_html_node_unwrap(rb_self.as_raw()) };
    serialize(&ruby, node, true, pretty)
}

/// `mkr_init_serialize` - the same entry point Init_makiri already calls.
///
/// # Safety
/// Called from `Init_makiri`, on the Ruby thread with the GVL held, after the
/// classes and modules exist.
pub unsafe extern "C" fn mkr_init_serialize() {
    let m = html_node_methods();
    for name in ["to_html", "to_s", "outer_html"] {
        m.define_method(name, method!(to_html, -1))
            .expect("defining an HTML serializer method");
    }
    m.define_method("inner_html", method!(inner_html, -1))
        .expect("defining an HTML serializer method");
}
