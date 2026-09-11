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
