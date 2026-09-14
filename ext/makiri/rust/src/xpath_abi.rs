//! The C types of the XPath engine, and the C functions it calls back into.
//!
//! Every layout here mirrors a declaration in ext/makiri/xpath/mkr_xpath.h or
//! mkr_xpath_internal.h. While that header is still compiled, `mkr_xpath_rs_sizes`
//! below hands the sizes back so the C side can check them against its own
//! `sizeof` rather than trusting this file (ext/makiri/xpath/mkr_xpath_rs_check.c).
//!
//! Standing alone there is no second declaration to disagree with: these types
//! ARE the layout, so the reporter has no reader and is not compiled. (Lexbor's
//! types are the opposite case - they belong to a vendored dependency, so the
//! checks in `lexbor_abi::agree` stay in every configuration.)
//!
//! It sits at the crate root rather than inside `xpath` because the glue needs
//! these types too - the XML query entry points hold an error, a value and a
//! limits pointer - and two Rust copies of one C layout is the drift that check
//! exists to catch. The engine reaches it as `xpath::abi`.

use core::ffi::{c_char, c_int, c_void};

/* ---- status / kind enums (C enums, so plain i32 / u32 values) ---- */

pub const XP_OK: c_int = 0;
/// Names the CSS lowering EMITS and the evaluator RESOLVES for an untyped
/// `:*-of-type`, where the "type" is the element's own expanded name - a
/// self-reference XPath 1.0 cannot express (there is no `current()`). The
/// leading \x01 cannot come out of the lexer, so these are unreachable from a
/// user expression. XML host only.
///
/// They live here, not beside the evaluator: one end emits them and the other
/// resolves them, and a name that only one end knows is a call that resolves to
/// nothing.
pub const FN_OF_TYPE_POS: &[u8] = b"\x01of-type-pos";
pub const FN_OF_TYPE_POS_LAST: &[u8] = b"\x01of-type-pos-last";

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
#[derive(Clone, Copy)]
pub struct OwnedText {
    pub ptr: *mut c_char,
    pub len: usize,
}

/// mkr_verified_text_t / mkr_borrowed_text_t - same layout, different contract.
#[derive(Clone, Copy)]
pub struct VerifiedText {
    pub ptr: *const c_char,
    pub len: usize,
}

/* ---- error / limits (mkr_xpath.h) ---- */

pub struct Error {
    pub status: c_int,
    pub message: *mut c_char,
}

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

#[derive(Clone, Copy)]
pub struct NodeSet {
    pub items: *mut *mut c_void,
    pub count: usize,
    pub capacity: usize,
}

#[derive(Clone, Copy)]
pub union ValU {
    pub nodeset: NodeSet,
    pub string: OwnedText,
    pub number: f64,
    pub boolean: c_int,
}

#[derive(Clone, Copy)]
pub struct PublicNodeSet {
    /// The same array as `NodeSet.items`; the public type names it `nodes`.
    pub nodes: *mut *mut c_void,
    pub count: usize,
}

#[derive(Clone, Copy)]
pub union XPathValueU {
    pub nodeset: PublicNodeSet,
    pub string: OwnedText,
    pub number: f64,
    pub boolean: c_int,
}

/// `mkr_xpath_value_t` - the result the glue receives. Distinct from `Val`: the
/// node-set arm carries no capacity, because ownership of the array transfers.
pub struct XPathValue {
    pub type_: u32,
    pub u: XPathValueU,
}

/// mkr_val_t - the engine's internal value, embedded in a node's memo slot.
#[derive(Clone, Copy)]
pub struct Val {
    pub type_: u32,
    pub u: ValU,
}

#[derive(Clone, Copy)]
pub struct NodeTest {
    pub kind: u32,
    pub prefix: OwnedText,
    pub local: OwnedText,
    pub pi_target: OwnedText,
}

#[derive(Clone, Copy)]
pub struct Step {
    pub axis: u32,
    pub test: NodeTest,
    pub predicates: *mut *mut Node,
    pub npredicates: usize,
}

#[derive(Clone, Copy)]
pub struct VarRef {
    pub prefix: OwnedText,
    pub name: OwnedText,
}

#[derive(Clone, Copy)]
pub struct FnCall {
    pub prefix: OwnedText,
    pub name: OwnedText,
    pub args: *mut *mut Node,
    pub nargs: usize,
}

#[derive(Clone, Copy)]
pub struct Unary {
    pub expr: *mut Node,
}

#[derive(Clone, Copy)]
pub struct BinOp {
    pub op: u32,
    pub lhs: *mut Node,
    pub rhs: *mut Node,
}

#[derive(Clone, Copy)]
pub struct Path {
    pub absolute: c_int,
    pub steps: *mut Step,
    pub nsteps: usize,
}

#[derive(Clone, Copy)]
pub struct Filter {
    pub expr: *mut Node,
    pub preds: *mut *mut Node,
    pub npreds: usize,
    pub path_steps: *mut Step,
    pub npath: usize,
}

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
pub struct Node {
    pub kind: u32,
    pub is_context_independent: u8,
    pub memoized: u8,
    pub memo_value: Val,
    pub u: NodeU,
}

/* ---- the C functions the front end calls back into ---- */

pub use crate::falloc::calloc::mkr_grow_reserve;
pub use crate::falloc::calloc::mkr_strndup;
pub use crate::xpath::ast_ops::mkr_apply_peephole;
pub use crate::xpath::ast_ops::mkr_mark_context_independent;
pub use crate::xpath::ast_ops::mkr_node_alloc;
pub use crate::xpath::ast_ops::mkr_node_free;
pub use crate::xpath::ast_ops::mkr_step_clear;
pub use crate::xpath::limits::mkr_limit_ast_node;
pub use crate::xpath::limits::mkr_limit_check_expr_bytes;
pub use crate::xpath::limits::mkr_limit_check_func_args;
pub use crate::xpath::limits::mkr_limit_check_predicates;
pub use crate::xpath::limits::mkr_limit_check_steps;
pub use crate::xpath::limits::mkr_limit_recurse_enter;
pub use crate::xpath::limits::mkr_limit_recurse_leave;

/* ---- the engine's runtime structures (mkr_xpath_internal.h, core/mkr_buf.h) ---- */

/// The engine's context. It used to be an opaque `_private: [u8; 0]` here and a
/// real struct in `xpath::ctx`, reconciled only by the linker seeing one C name;
/// with no C ABI between them that is two types, so this IS the one type.
pub use crate::xpath::ctx::Context;

/// `mkr_buf_t` - a growable byte buffer with a byte ceiling. Declared in
/// `crate::cbuf`, which is where the C layout lives now that the glue writes
/// into one too.
pub(crate) use crate::cbuf::{mkr_buf_append, Buf};

pub struct StrCacheEntry {
    pub node: *mut c_void,
    pub str_: *mut c_char,
    pub len: usize,
}

/// `mkr_str_cache_t` - the per-evaluate node string-value cache: an ordered
/// store plus a pointer-keyed open-addressing index into it.
pub struct StrCache {
    pub entries: *mut StrCacheEntry,
    pub count: usize,
    pub cap: usize,
    /// node pointer -> entry index + 1; 0 is an empty slot.
    pub buckets: *mut usize,
    pub bucket_cap: usize,
    pub total_bytes: usize,
}

pub struct OrderBucket {
    /// NULL is an empty slot.
    pub node: *const c_void,
    pub ord: usize,
}

/// `mkr_doc_order_index_t` - the per-evaluate document-order index.
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

/// Tag-index hooks (HTML only): `lookup` returns the document-ordered bucket of
/// elements whose tag id matches.
pub type TagIndexLookup = Option<
    unsafe extern "C" fn(
        index: *const c_void,
        tag_id: usize,
        count: *mut usize,
    ) -> *const *mut c_void,
>;
pub type TagIndexForeign = Option<unsafe extern "C" fn(index: *const c_void) -> c_int>;

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

pub use crate::xpath::ctx::mkr_ctx_document;
pub use crate::xpath::ctx::mkr_ctx_element_index;
pub use crate::xpath::ctx::mkr_ctx_func_resolver;
pub use crate::xpath::ctx::mkr_ctx_limits;
pub use crate::xpath::ctx::mkr_ctx_lookup_ns;
pub use crate::xpath::ctx::mkr_ctx_lookup_variable_text;
pub use crate::xpath::ctx::mkr_ctx_name_index_get;
pub use crate::xpath::ctx::mkr_ctx_name_index_lookup;
pub use crate::xpath::ctx::mkr_ctx_name_index_owner;
pub use crate::xpath::ctx::mkr_ctx_node;
pub use crate::xpath::ctx::mkr_ctx_order_index;
pub use crate::xpath::ctx::mkr_ctx_str_cache;
pub use crate::xpath::ctx::mkr_ctx_tag_has_foreign;
pub use crate::xpath::ctx::mkr_ctx_tag_lookup;
pub use crate::xpath::ctx::mkr_ctx_unprefixed_lax;
pub use crate::xpath::ctx::mkr_xpath_get_user_data;
pub use crate::xpath::limits::mkr_limit_check_nodeset_size;
pub use crate::xpath::limits::mkr_limit_check_string_bytes;
pub use crate::xpath::limits::mkr_limit_eval_op;
pub use crate::xpath::runtime_abi::mkr_doc_order_index_clear;
pub use crate::xpath::runtime_abi::mkr_nodeset_clear;
pub use crate::xpath::runtime_abi::mkr_nodeset_init;
pub use crate::xpath::runtime_abi::mkr_nodeset_push;
pub use crate::xpath::runtime_abi::mkr_owned_text_clear;
pub use crate::xpath::runtime_abi::mkr_owned_text_from_borrowed_copy;
pub use crate::xpath::runtime_abi::mkr_str_cache_index_put;
pub use crate::xpath::runtime_abi::mkr_str_cache_reindex;
pub use crate::xpath::runtime_abi::mkr_val_clear;
pub use crate::xpath::runtime_abi::mkr_val_set_owned_text;

extern "C" {}

/* The C/Ruby-facing cleanup entry points live at the explicit raw boundary. */
pub use crate::xpath::boundary::{mkr_err_set, mkr_xpath_error_clear, mkr_xpath_value_clear};

/// `mkr_ptr_hash` (core/mkr_hash.h) - the MurmurHash3 fmix64 finalizer.
///
/// Written out because C's is `static inline`, and it belongs with the C
/// declarations rather than with the tables that use it: the string-value
/// cache's open-addressing index is filled by `mkr_str_cache_index_put` on the
/// C side and probed here, so the two hashes have to agree bit for bit. That
/// makes it an ABI fact, not a hashing choice.
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
