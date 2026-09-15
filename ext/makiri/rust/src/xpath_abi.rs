//! The XPath engine's shared data layouts - the AST, values, node-sets, limits
//! and errors - and the construction entry points the front ends build with.
//!
//! These are Rust types that keep the field layout the C engine had. Nothing
//! outside the crate reads them (the extension exports only `Init_makiri`), so
//! the layout is not an ABI. It stays because every allocation behind these
//! types goes through `falloc`, which is what lets `rake oom` fail each one and
//! the engine raise instead of aborting; a `Box` or `Vec` AST would abort the
//! process on OOM. Ownership on the Rust side is `xpath::own`'s guards.
//!
//! It sits at the crate root rather than inside `xpath` because the glue and the
//! CSS lowering use these types too. The engine reaches it as `xpath::abi`.

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

/* ---- text ---- */

/// The borrowed views live in `crate::text`; re-exported so the engine's
/// `super::abi::*` imports see them beside the owned slot.
pub use crate::text::{BorrowedText, VerifiedText};

/// The raw slot for an engine-owned UTF-8 byte string, NUL-terminated in its
/// backing allocation.
///
/// Interior NULs are possible - DOM text may hold U+0000 - so it is read as
/// `(ptr, len)` and borrowed as a [`BorrowedText`], never a [`VerifiedText`].
///
/// This is a slot, not an owner: it is `Copy` because the AST and value unions
/// hold it, and clearing one copy leaves the others dangling. Code that owns
/// text outside those layouts holds a [`crate::xpath::own::OwnedText`].
#[derive(Clone, Copy)]
pub struct TextSlot {
    ptr: *mut c_char,
    len: usize,
}

impl TextSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            ptr: core::ptr::null_mut(),
            len: 0,
        }
    }

    /// Construct a raw-owned value at the allocator/runtime boundary.
    ///
    /// # Safety
    /// `ptr` must be null or point to `len` live bytes followed by a NUL byte,
    /// allocated by the allocator used by `mkr_owned_text_clear`.
    pub(crate) unsafe fn from_raw_parts(ptr: *mut c_char, len: usize) -> Self {
        Self { ptr, len }
    }

    pub(crate) const fn as_ptr(self) -> *mut c_char {
        self.ptr
    }

    pub(crate) const fn len(self) -> usize {
        self.len
    }

    /// Whether this slot represents an omitted value rather than an empty
    /// allocated string.
    pub(crate) const fn is_absent(self) -> bool {
        self.ptr.is_null()
    }

    pub(crate) const fn is_present(self) -> bool {
        !self.is_absent()
    }

    /// Whether the string has no content. An absent slot is empty by content,
    /// but remains distinguishable through [`Self::is_absent`].
    pub(crate) const fn is_empty(self) -> bool {
        self.is_absent() || self.len == 0
    }

    pub(crate) unsafe fn as_bytes<'a>(self) -> &'a [u8] {
        if self.is_empty() {
            &[]
        } else {
            core::slice::from_raw_parts(self.ptr as *const u8, self.len)
        }
    }
}

/* ---- error / limits ---- */

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

/* ---- the AST ---- */

#[derive(Clone, Copy)]
pub struct NodeSet {
    pub items: *mut *mut c_void,
    pub count: usize,
    pub capacity: usize,
}

#[derive(Clone, Copy)]
pub union ValU {
    pub nodeset: NodeSet,
    pub string: TextSlot,
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
    pub string: TextSlot,
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
    pub prefix: TextSlot,
    pub local: TextSlot,
    pub pi_target: TextSlot,
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
    pub prefix: TextSlot,
    pub name: TextSlot,
}

#[derive(Clone, Copy)]
pub struct FnCall {
    pub prefix: TextSlot,
    pub name: TextSlot,
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
    pub literal: TextSlot,
    pub literal_num: f64,
    pub varref: VarRef,
    pub fncall: FnCall,
    pub unary: Unary,
    pub binop: BinOp,
    pub path: Path,
    pub filter: Filter,
}

/// The compiled AST node. Allocated zeroed by `node_alloc` and freed by
/// `mkr_node_free`, both in `xpath::ast_ops`.
pub struct Node {
    pub kind: u32,
    pub is_context_independent: u8,
    pub memoized: u8,
    pub memo_value: Val,
    pub u: NodeU,
}

/* ---- the construction entry points the front ends build with ---- */

pub use crate::falloc::calloc::mkr_grow_reserve;
pub use crate::falloc::calloc::mkr_strndup;
pub use crate::xpath::ast_ops::mkr_apply_peephole;
pub use crate::xpath::ast_ops::mkr_mark_context_independent;
pub use crate::xpath::ast_ops::mkr_node_free;
pub use crate::xpath::ast_ops::mkr_step_clear;
pub(crate) use crate::xpath::ast_ops::node_alloc;
pub use crate::xpath::limits::mkr_limit_ast_node;
pub use crate::xpath::limits::mkr_limit_check_expr_bytes;
pub use crate::xpath::limits::mkr_limit_check_func_args;
pub use crate::xpath::limits::mkr_limit_check_predicates;
pub use crate::xpath::limits::mkr_limit_check_steps;
pub use crate::xpath::limits::mkr_limit_recurse_enter;
pub use crate::xpath::limits::mkr_limit_recurse_leave;

/* ---- the engine's runtime structures ---- */

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
pub use crate::xpath::runtime_abi::mkr_str_cache_index_put;
pub use crate::xpath::runtime_abi::mkr_str_cache_reindex;
pub use crate::xpath::runtime_abi::mkr_val_clear;
pub use crate::xpath::runtime_abi::mkr_val_set_owned_text;

/* The cleanup entry points the glue calls live at the raw boundary. */
pub use crate::xpath::boundary::{mkr_err_set, mkr_xpath_error_clear, mkr_xpath_value_clear};

/// The proof a failure's message was written; see `xpath::msg`.
pub use crate::xpath::msg::Reported;

/// The MurmurHash3 fmix64 finalizer over a pointer value.
///
/// One definition for every pointer-keyed table: the string-value cache's index
/// is filled by `mkr_str_cache_index_put` and probed by its readers, and the
/// text index uses it too, so all of them must hash the same way.
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
