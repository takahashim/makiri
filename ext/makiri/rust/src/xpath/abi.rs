//! The C types the XPath front end shares with the rest of the engine, and the
//! C functions it calls back into.
//!
//! Every layout here mirrors a declaration in ext/makiri/xpath/mkr_xpath.h or
//! mkr_xpath_internal.h. `mkr_xpath_rs_sizes` below hands the sizes back so the
//! C side can check them against its own `sizeof` rather than trusting this file
//! (see ext/makiri/xpath/mkr_xpath_rs_check.c).

use core::ffi::{c_char, c_int, c_void};

/* ---- status / kind enums (C enums, so plain i32 / u32 values) ---- */

pub const XP_OK: c_int = 0;
pub const XP_ERR_SYNTAX: c_int = 2;
pub const XP_ERR_INTERNAL: c_int = 5;
pub const XP_ERR_OOM: c_int = 6;
pub const XP_ERR_LIMIT: c_int = 7;
pub const XP_ERR_TYPE: c_int = 3;
pub const XP_ERR_RUNTIME: c_int = 4;
pub const XP_ERR_NOT_IMPLEMENTED: c_int = 1;

pub const MKR_OK: c_int = 0;

/* mkr_nk_t */
pub const NK_LITERAL_STR: u32 = 0;
pub const NK_LITERAL_NUM: u32 = 1;
pub const NK_VARREF: u32 = 2;
pub const NK_FNCALL: u32 = 3;
pub const NK_UNARY: u32 = 4;
pub const NK_BINOP: u32 = 5;
pub const NK_PATH: u32 = 6;
pub const NK_FILTER: u32 = 7;

/* mkr_axis_t */
pub const AXIS_CHILD: u32 = 0;
pub const AXIS_DESCENDANT: u32 = 1;
pub const AXIS_PARENT: u32 = 2;
pub const AXIS_ANCESTOR: u32 = 3;
pub const AXIS_FOLLOWING_SIBLING: u32 = 4;
pub const AXIS_PRECEDING_SIBLING: u32 = 5;
pub const AXIS_FOLLOWING: u32 = 6;
pub const AXIS_PRECEDING: u32 = 7;
pub const AXIS_ATTRIBUTE: u32 = 8;
pub const AXIS_NAMESPACE: u32 = 9;
pub const AXIS_SELF: u32 = 10;
pub const AXIS_DESCENDANT_OR_SELF: u32 = 11;
pub const AXIS_ANCESTOR_OR_SELF: u32 = 12;

/* mkr_nt_kind_t */
pub const NT_NAME: u32 = 0;
pub const NT_WILDCARD: u32 = 1;
pub const NT_NODE: u32 = 2;
pub const NT_TEXT: u32 = 3;
pub const NT_COMMENT: u32 = 4;
pub const NT_PI: u32 = 5;

/* mkr_op_t */
pub const OP_OR: u32 = 0;
pub const OP_AND: u32 = 1;
pub const OP_EQ: u32 = 2;
pub const OP_NE: u32 = 3;
pub const OP_LT: u32 = 4;
pub const OP_GT: u32 = 5;
pub const OP_LE: u32 = 6;
pub const OP_GE: u32 = 7;
pub const OP_ADD: u32 = 8;
pub const OP_SUB: u32 = 9;
pub const OP_MUL: u32 = 10;
pub const OP_DIV: u32 = 11;
pub const OP_MOD: u32 = 12;
pub const OP_UNION: u32 = 13;

/* ---- text views (core/mkr_text.h) ---- */

/// mkr_owned_text_t - owned, NUL-terminated at ptr[len].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OwnedText {
    pub ptr: *mut c_char,
    pub len: usize,
}

/// mkr_verified_text_t / mkr_borrowed_text_t - same layout, different contract.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VerifiedText {
    pub ptr: *const c_char,
    pub len: usize,
}

/* ---- error / limits (mkr_xpath.h) ---- */

#[repr(C)]
pub struct Error {
    pub status: c_int,
    pub message: *mut c_char,
}

#[repr(C)]
pub struct Limits {
    pub max_expr_bytes: usize,
    pub max_ast_nodes: usize,
    pub max_steps: usize,
    pub max_predicates: usize,
    pub max_function_args: usize,
    pub max_nodeset_size: usize,
    pub max_eval_ops: usize,
    pub max_string_bytes: usize,
    pub max_recursion_depth: usize,
    pub ast_nodes: usize,
    pub eval_ops: usize,
    pub recursion_depth: usize,
}

/* ---- the AST (mkr_xpath_internal.h) ---- */

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NodeSet {
    pub items: *mut *mut c_void,
    pub count: usize,
    pub capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union ValU {
    pub nodeset: NodeSet,
    pub string: OwnedText,
    pub number: f64,
    pub boolean: c_int,
}

/// mkr_val_t - the engine's internal value, embedded in a node's memo slot.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Val {
    pub type_: u32,
    pub u: ValU,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NodeTest {
    pub kind: u32,
    pub prefix: OwnedText,
    pub local: OwnedText,
    pub pi_target: OwnedText,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Step {
    pub axis: u32,
    pub test: NodeTest,
    pub predicates: *mut *mut Node,
    pub npredicates: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VarRef {
    pub prefix: OwnedText,
    pub name: OwnedText,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FnCall {
    pub prefix: OwnedText,
    pub name: OwnedText,
    pub args: *mut *mut Node,
    pub nargs: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Unary {
    pub expr: *mut Node,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BinOp {
    pub op: u32,
    pub lhs: *mut Node,
    pub rhs: *mut Node,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Path {
    pub absolute: c_int,
    pub steps: *mut Step,
    pub nsteps: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Filter {
    pub expr: *mut Node,
    pub preds: *mut *mut Node,
    pub npreds: usize,
    pub path_steps: *mut Step,
    pub npath: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union NodeU {
    pub literal: OwnedText,
    pub literal_num: f64,
    pub varref: VarRef,
    pub fncall: FnCall,
    pub unary: Unary,
    pub binop: BinOp,
    pub path: Path,
    pub filter: Filter,
}

/// struct mkr_node_s - the compiled AST node. Allocated zeroed by
/// `mkr_node_alloc` and freed by `mkr_node_free`, both on the C side.
#[repr(C)]
pub struct Node {
    pub kind: u32,
    pub is_context_independent: u8,
    pub memoized: u8,
    pub memo_value: Val,
    pub u: NodeU,
}

/* ---- the C functions the front end calls back into ---- */

extern "C" {
    /// Charges max_ast_nodes, then returns a zeroed node with its kind set, or
    /// NULL with *err set. The one AST factory, shared with the CSS lowering.
    pub fn mkr_node_alloc(limits: *mut Limits, err: *mut Error, kind: u32) -> *mut Node;
    pub fn mkr_node_free(n: *mut Node);
    pub fn mkr_step_clear(s: *mut Step);

    pub fn mkr_err_set(err: *mut Error, status: c_int, msg: *const c_char);

    pub fn mkr_limit_recurse_enter(l: *mut Limits, err: *mut Error) -> c_int;
    pub fn mkr_limit_recurse_leave(l: *mut Limits);
    pub fn mkr_limit_check_steps(l: *mut Limits, nsteps: usize, err: *mut Error) -> c_int;
    pub fn mkr_limit_check_predicates(l: *mut Limits, npreds: usize, err: *mut Error) -> c_int;
    pub fn mkr_limit_check_func_args(l: *mut Limits, nargs: usize, err: *mut Error) -> c_int;
    pub fn mkr_limit_check_expr_bytes(l: *mut Limits, bytes: usize, err: *mut Error) -> c_int;

    pub fn mkr_apply_peephole(n: *mut Node);
    pub fn mkr_mark_context_independent(n: *mut Node);

    pub fn mkr_strndup(s: *const c_char, n: usize) -> *mut c_char;
    pub fn mkr_grow_reserve(
        ptr: *mut *mut c_void,
        cap: *mut usize,
        need: usize,
        elem: usize,
    ) -> c_int;
}

/// Bytes as text for a message, with anything non-ASCII-printable escaped, so a
/// name echoed back into an error cannot carry control bytes into the message.
pub struct Bytes<'a>(pub &'a [u8]);

impl core::fmt::Display for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for &b in self.0 {
            if (0x20..0x7F).contains(&b) {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{:02x}", b)?;
            }
        }
        Ok(())
    }
}

/// A NUL-terminated message assembled on the stack. Error paths must not
/// allocate - one of them reports OOM - so this is where messages are built,
/// and it truncates rather than growing.
pub struct MsgBuf {
    buf: [u8; 200],
    len: usize,
}

impl Default for MsgBuf {
    fn default() -> Self {
        MsgBuf { buf: [0; 200], len: 0 }
    }
}

impl MsgBuf {
    pub fn as_ptr(&self) -> *const c_char {
        self.buf.as_ptr() as *const c_char
    }
}

impl core::fmt::Write for MsgBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        /* Leave one byte for the terminator, and cut on a char boundary. */
        let room = self.buf.len() - 1 - self.len;
        let mut n = s.len().min(room);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Set `err` from a formatted message. `mkr_err_set` copies it (mkr_xpath.c),
/// so the stack buffer does not outlive the call.
///
/// Crate-internal, and the one place the front end writes an error: every
/// caller already holds the `*mut Error` the C caller handed it, and passing a
/// NULL or a dangling one would be the caller's bug either way.
pub(crate) fn err_set_fmt(err: *mut Error, status: c_int, args: core::fmt::Arguments<'_>) {
    use core::fmt::Write;
    let mut m = MsgBuf::default();
    let _ = m.write_fmt(args);
    unsafe { mkr_err_set(err, status, m.as_ptr()) }
}

/// `mkr_err_setf` for the Rust side: `err_setf!(err, status, "...", args)`.
#[macro_export]
macro_rules! err_setf {
    ($err:expr, $status:expr, $($arg:tt)*) => {
        $crate::xpath::abi::err_set_fmt($err, $status, format_args!($($arg)*))
    };
}

/// The sizes C checks its own `sizeof` against, so a field added on one side
/// without the other is a build-time failure rather than silent corruption.
#[no_mangle]
pub unsafe extern "C" fn mkr_xpath_rs_sizes(out: *mut usize, cap: usize) -> usize {
    let sizes = [
        core::mem::size_of::<Node>(),
        core::mem::size_of::<Step>(),
        core::mem::size_of::<NodeTest>(),
        core::mem::size_of::<Val>(),
        core::mem::size_of::<NodeU>(),
        core::mem::size_of::<Limits>(),
        core::mem::size_of::<Error>(),
        core::mem::size_of::<VerifiedText>(),
    ];
    if !out.is_null() {
        for (i, s) in sizes.iter().enumerate().take(cap) {
            *out.add(i) = *s;
        }
    }
    sizes.len()
}

/* ---- the engine's runtime structures (mkr_xpath_internal.h, core/mkr_buf.h) ---- */

/// `mkr_xpath_context_s`, opaque: the engine reaches it only through the
/// `mkr_ctx_*` accessors, exactly as the C bodies do.
#[repr(C)]
pub struct Context {
    _private: [u8; 0],
}

/// `mkr_buf_t` - a growable byte buffer with a byte ceiling. `init` and `free`
/// are `static inline` in C, so they are written out here.
#[repr(C)]
pub struct Buf {
    pub data: *mut c_char,
    pub len: usize,
    pub cap: usize,
    /// 0 selects the conservative default ceiling; it is not "unbounded".
    pub max: usize,
}

impl Buf {
    pub fn new(max: usize) -> Buf {
        Buf { data: core::ptr::null_mut(), len: 0, cap: 0, max }
    }
    /// # Safety
    /// Must not be called twice on the same buffer, or after `mkr_buf_steal`.
    pub unsafe fn free(&mut self) {
        if !self.data.is_null() {
            libc_free(self.data as *mut c_void);
            self.data = core::ptr::null_mut();
        }
        self.len = 0;
        self.cap = 0;
    }
}

#[repr(C)]
pub struct StrCacheEntry {
    pub node: *mut c_void,
    pub str_: *mut c_char,
    pub len: usize,
}

/// `mkr_str_cache_t` - the per-evaluate node string-value cache: an ordered
/// store plus a pointer-keyed open-addressing index into it.
#[repr(C)]
pub struct StrCache {
    pub entries: *mut StrCacheEntry,
    pub count: usize,
    pub cap: usize,
    /// node pointer -> entry index + 1; 0 is an empty slot.
    pub buckets: *mut usize,
    pub bucket_cap: usize,
    pub total_bytes: usize,
}

#[repr(C)]
pub struct OrderBucket {
    /// NULL is an empty slot.
    pub node: *const c_void,
    pub ord: usize,
}

/// `mkr_doc_order_index_t` - the per-evaluate document-order index.
#[repr(C)]
pub struct OrderIndex {
    pub buckets: *mut OrderBucket,
    pub cap: usize,
    pub count: usize,
    pub built: c_int,
}

/// The custom-function resolver the glue installs for a Ruby handler.
pub type FuncResolver = Option<
    unsafe extern "C" fn(
        user_data: *mut c_void,
        ctx: *mut Context,
        self_node: *mut c_void,
        self_pos: usize,
        self_size: usize,
        ns_uri: *const c_char,
        local_name: *const c_char,
        args: *mut c_void,
        nargs: usize,
        out: *mut c_void,
        err: *mut Error,
    ) -> c_int,
>;

/// Name-index hooks (XML only): `get` lazily builds and caches the index on the
/// owning document, `lookup` returns the document-ordered bucket for a name.
pub type NameIndexGet = Option<unsafe extern "C" fn(owner: *mut c_void) -> *mut c_void>;
pub type NameIndexLookup = Option<
    unsafe extern "C" fn(
        index: *const c_void,
        local: *const c_char,
        local_len: usize,
        ns_uri: *const c_char,
        ns_uri_len: usize,
        count: *mut usize,
    ) -> *const *mut c_void,
>;

extern "C" {
    /* buffers */
    pub fn mkr_buf_append(b: *mut Buf, bytes: *const c_void, n: usize) -> c_int;
    pub fn mkr_buf_steal(b: *mut Buf, out_len: *mut usize) -> *mut c_char;

    /* owned text */
    pub fn mkr_owned_text_clear(t: *mut OwnedText);
    pub fn mkr_owned_text_from_borrowed_copy(
        out: *mut OwnedText,
        t: VerifiedText,
        err: *mut Error,
        what: *const c_char,
    ) -> c_int;

    /* values and node-sets */
    pub fn mkr_val_clear(v: *mut Val);
    pub fn mkr_val_set_owned_text(v: *mut Val, text: OwnedText);
    pub fn mkr_nodeset_init(ns: *mut NodeSet);
    pub fn mkr_nodeset_push(
        ns: *mut NodeSet,
        node: *mut c_void,
        limits: *mut Limits,
        err: *mut Error,
    ) -> c_int;
    pub fn mkr_nodeset_clear(ns: *mut NodeSet);

    /* limits not already declared above */
    pub fn mkr_limit_eval_op(l: *mut Limits, err: *mut Error) -> c_int;
    pub fn mkr_limit_check_nodeset_size(l: *mut Limits, n: usize, err: *mut Error) -> c_int;
    pub fn mkr_limit_check_string_bytes(l: *mut Limits, bytes: usize, err: *mut Error) -> c_int;

    /* errors */
    pub fn mkr_err_setf(err: *mut Error, status: c_int, fmt: *const c_char, ...);
    pub fn mkr_xpath_error_clear(e: *mut Error);

    /* context accessors */
    pub fn mkr_ctx_limits(ctx: *mut Context) -> *mut Limits;
    pub fn mkr_ctx_document(ctx: *mut Context) -> *mut c_void;
    pub fn mkr_ctx_node(ctx: *mut Context) -> *mut c_void;
    pub fn mkr_ctx_str_cache(ctx: *mut Context) -> *mut StrCache;
    pub fn mkr_ctx_order_index(ctx: *mut Context) -> *mut OrderIndex;
    pub fn mkr_ctx_unprefixed_lax(ctx: *mut Context) -> c_int;
    pub fn mkr_ctx_func_resolver(ctx: *mut Context) -> FuncResolver;
    pub fn mkr_xpath_get_user_data(ctx: *mut Context) -> *mut c_void;
    pub fn mkr_ctx_lookup_ns(
        ctx: *mut Context,
        prefix: *const c_char,
        prefix_len: usize,
        out_uri_len: *mut usize,
    ) -> *const c_char;
    pub fn mkr_ctx_lookup_variable_text(
        ctx: *mut Context,
        prefix: *const c_char,
        prefix_len: usize,
        name: *const c_char,
        name_len: usize,
        out: *mut VerifiedText,
    ) -> c_int;

    /* element-name index (XML) */
    pub fn mkr_ctx_name_index_owner(ctx: *mut Context) -> *mut c_void;
    pub fn mkr_ctx_name_index_get(ctx: *mut Context) -> NameIndexGet;
    pub fn mkr_ctx_name_index_lookup(ctx: *mut Context) -> NameIndexLookup;

    /* the string-value cache's index bookkeeping stays in C, so both sides
     * drive one open-addressing table */
    pub fn mkr_str_cache_index_put(c: *mut StrCache, idx: usize);
    pub fn mkr_str_cache_reindex(c: *mut StrCache, bucket_cap: usize) -> c_int;

    pub fn mkr_doc_order_index_clear(idx: *mut OrderIndex);

    /* allocation */
    pub fn mkr_reallocarray(ptr: *mut c_void, count: usize, elem: usize) -> *mut c_void;
    pub fn mkr_callocarray(count: usize, elem: usize) -> *mut c_void;

    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

/// `mkr_ptr_hash` (core/mkr_hash.h) - the MurmurHash3 fmix64 finalizer. Written
/// out because C's is `static inline`, and it has to agree bit for bit: the
/// string-value cache's index is built by both sides.
#[inline]
pub fn ptr_hash<T>(p: *const T) -> u64 {
    let mut h = p as usize as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    h
}
