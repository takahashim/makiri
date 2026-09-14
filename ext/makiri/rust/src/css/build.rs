//! AST builders shared by the lowering.
//!
//! Every one takes ownership of the nodes it is handed and frees them on any
//! failure, so a caller can chain them without tracking partial state - which is
//! what the C's cascade of `if (x == NULL) { free(...); return NULL; }` was
//! doing, spelled once per builder instead of once per call.
//!
//! The allocations match `mkr_node_free`'s contract exactly: nodes through
//! `mkr_node_alloc`, owned text through `mkr_owned_text_from_borrowed_copy`,
//! arrays through the C allocator. That is why none of this uses `falloc` or a
//! `Vec` - the C's free function is what eventually runs over it.

use core::ffi::c_void;

use super::Build;
use crate::falloc::calloc::mkr_reallocarray;
use crate::xpath_abi::{
    mkr_node_alloc, mkr_node_free, mkr_owned_text_from_borrowed_copy, Node, OwnedText, Step,
    VerifiedText, NK_BINOP, NK_FNCALL, NK_LITERAL_NUM, NK_LITERAL_STR, NK_PATH, NT_NAME,
};

use crate::falloc::calloc::mkr_callocarray;
pub use crate::xpath::ast_ops::mkr_step_clear;
pub use crate::xpath::runtime_abi::mkr_owned_text_clear;

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

#[inline]
fn borrowed(s: &[u8]) -> VerifiedText {
    VerifiedText {
        ptr: s.as_ptr() as *const core::ffi::c_char,
        len: s.len(),
    }
}

/// A zeroed node of `kind`, charged against the AST budget.
pub(crate) unsafe fn node(b: &Build, kind: u32) -> *mut Node {
    mkr_node_alloc(b.limits, b.err, kind)
}

/// Copy `s` into an owned text slot. `false` on failure, with `*err` set.
pub(crate) unsafe fn set_text(b: &Build, out: *mut OwnedText, s: &[u8]) -> bool {
    mkr_owned_text_from_borrowed_copy(out, borrowed(s), b.err, c"css name".as_ptr()) == 0
}

pub(crate) unsafe fn literal(b: &Build, s: &[u8]) -> *mut Node {
    let n = node(b, NK_LITERAL_STR);
    if n.is_null() {
        return n;
    }
    if !set_text(b, &mut (*n).u.literal, s) {
        mkr_node_free(n);
        return core::ptr::null_mut();
    }
    n
}

pub(crate) unsafe fn num(b: &Build, v: f64) -> *mut Node {
    let n = node(b, NK_LITERAL_NUM);
    if !n.is_null() {
        (*n).u.literal_num = v;
    }
    n
}

/// `lhs op rhs`, taking ownership of both. A NULL operand frees the other and
/// answers NULL, so a failure anywhere in a nested build unwinds by itself.
pub(crate) unsafe fn binop(b: &Build, op: u32, lhs: *mut Node, rhs: *mut Node) -> *mut Node {
    if lhs.is_null() || rhs.is_null() {
        mkr_node_free(lhs);
        mkr_node_free(rhs);
        return core::ptr::null_mut();
    }
    let n = node(b, NK_BINOP);
    if n.is_null() {
        mkr_node_free(lhs);
        mkr_node_free(rhs);
        return core::ptr::null_mut();
    }
    (*n).u.binop.op = op;
    (*n).u.binop.lhs = lhs;
    (*n).u.binop.rhs = rhs;
    n
}

/// An argument array of `n` slots, or NULL with `*err` set.
///
/// Zero slots still allocates one, because `mkr_callocarray(0, _)` answers NULL
/// and a NULL array would be indistinguishable from a failure.
pub(crate) unsafe fn args(b: &Build, n: usize) -> *mut *mut Node {
    let p = mkr_callocarray(
        if n == 0 { 1 } else { n },
        core::mem::size_of::<*mut Node>(),
    ) as *mut *mut Node;
    if p.is_null() {
        b.oom();
    }
    p
}

/// A call to an internal, compile-time-known function name, taking ownership of
/// `argv[0..nargs)`. Frees them on any failure.
pub(crate) unsafe fn fncall(
    b: &Build,
    name: &[u8],
    argv: *mut *mut Node,
    nargs: usize,
) -> *mut Node {
    if argv.is_null() {
        return core::ptr::null_mut();
    }
    let free_args = |argv: *mut *mut Node| {
        for i in 0..nargs {
            mkr_node_free(*argv.add(i));
        }
        libc_free(argv as *mut c_void);
    };
    for i in 0..nargs {
        if (*argv.add(i)).is_null() {
            free_args(argv);
            return core::ptr::null_mut();
        }
    }
    let n = node(b, NK_FNCALL);
    if n.is_null() {
        free_args(argv);
        return core::ptr::null_mut();
    }
    if !set_text(b, &mut (*n).u.fncall.name, name) {
        free_args(argv);
        mkr_node_free(n);
        return core::ptr::null_mut();
    }
    (*n).u.fncall.args = argv;
    (*n).u.fncall.nargs = nargs;
    n
}

/// A one-step relative PATH with no predicates: `axis::nodetest`.
///
/// `local` is `None` for a wildcard or a kind test. Used for `@attr`,
/// `preceding-sibling::*`, `child::node()` and the rest.
pub(crate) unsafe fn step_path(
    b: &Build,
    axis: u32,
    nt_kind: u32,
    local: Option<&[u8]>,
) -> *mut Node {
    named_step_path_inner(b, axis, None, local, nt_kind)
}

/// A one-step relative PATH with a NAME test: `axis::[prefix:]local`.
///
/// The shared builder for every named single-step path - `@prefix:name`, the
/// of-type `not()`, the nth-of-type position - so the step alloc, the two text
/// sets and the partial-free on failure exist once.
pub(crate) unsafe fn named_step_path(
    b: &Build,
    axis: u32,
    prefix: Option<&[u8]>,
    name: &[u8],
) -> *mut Node {
    named_step_path_inner(b, axis, prefix, Some(name), NT_NAME)
}

unsafe fn named_step_path_inner(
    b: &Build,
    axis: u32,
    prefix: Option<&[u8]>,
    local: Option<&[u8]>,
    nt_kind: u32,
) -> *mut Node {
    let n = node(b, NK_PATH);
    if n.is_null() {
        return n;
    }
    let steps = mkr_callocarray(1, core::mem::size_of::<Step>()) as *mut Step;
    if steps.is_null() {
        b.oom();
        mkr_node_free(n);
        return core::ptr::null_mut();
    }
    (*steps).axis = axis;
    (*steps).test.kind = nt_kind;

    if nt_kind == NT_NAME {
        if let Some(local) = local {
            if !set_text(b, &mut (*steps).test.local, local) {
                libc_free(steps as *mut c_void);
                mkr_node_free(n);
                return core::ptr::null_mut();
            }
        }
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            if !set_text(b, &mut (*steps).test.prefix, prefix) {
                mkr_owned_text_clear(&mut (*steps).test.local);
                libc_free(steps as *mut c_void);
                mkr_node_free(n);
                return core::ptr::null_mut();
            }
        }
    }

    (*n).u.path.absolute = 0;
    (*n).u.path.steps = steps;
    (*n).u.path.nsteps = 1;
    n
}

/// `@prefix:name` (or `@name`) as a relative attribute-axis path.
pub(crate) unsafe fn attr_ns(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> *mut Node {
    named_step_path(b, crate::xpath_abi::AXIS_ATTRIBUTE, prefix, name)
}

/// `@name` with no namespace.
pub(crate) unsafe fn attr(b: &Build, name: &[u8]) -> *mut Node {
    attr_ns(b, None, name)
}

/// A one-argument call, the shape most of the lowering wants.
pub(crate) unsafe fn call1(b: &Build, name: &[u8], a0: *mut Node) -> *mut Node {
    let a = args(b, 1);
    if a.is_null() {
        mkr_node_free(a0);
        return core::ptr::null_mut();
    }
    *a = a0;
    fncall(b, name, a, 1)
}

/// A two-argument call.
pub(crate) unsafe fn call2(b: &Build, name: &[u8], a0: *mut Node, a1: *mut Node) -> *mut Node {
    let a = args(b, 2);
    if a.is_null() {
        mkr_node_free(a0);
        mkr_node_free(a1);
        return core::ptr::null_mut();
    }
    *a = a0;
    *a.add(1) = a1;
    fncall(b, name, a, 2)
}

/// `normalize-space(@[prefix:]name)`.
pub(crate) unsafe fn norm_attr(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> *mut Node {
    call1(b, b"normalize-space", attr_ns(b, prefix, name))
}

/// `concat(" ", normalize-space(@name), " ")` - the whitespace-padded token list.
pub(crate) unsafe fn padded_tokens(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> *mut Node {
    let a = args(b, 3);
    if a.is_null() {
        return core::ptr::null_mut();
    }
    *a = literal(b, b" ");
    *a.add(1) = norm_attr(b, prefix, name);
    *a.add(2) = literal(b, b" ");
    fncall(b, b"concat", a, 3)
}

/// `contains(concat(' ', normalize-space(@name), ' '), ' value ')` - the
/// `[name~=value]` and `.class` membership predicate.
///
/// The value is padded with spaces so a token only matches whole, which is what
/// makes this equivalent to CSS's whitespace-separated list semantics.
pub(crate) unsafe fn token_match(
    b: &Build,
    prefix: Option<&[u8]>,
    attr_name: &[u8],
    value: &[u8],
) -> *mut Node {
    /* The padded literal is built on the Rust stack rather than in a C
     * allocation: it is copied into the AST by `literal`, so it needs to live
     * only until then. The C malloc'd it because it had no other way to
     * concatenate. */
    let mut padded = match crate::falloc::try_vec_with_capacity::<u8>(value.len() + 2) {
        Some(v) => v,
        None => {
            b.oom();
            return core::ptr::null_mut();
        }
    };
    padded.push(b' ');
    padded.extend_from_slice(value);
    padded.push(b' ');

    call2(
        b,
        b"contains",
        padded_tokens(b, prefix, attr_name),
        literal(b, &padded),
    )
}

/// Free a built-but-unattached step array.
pub(crate) unsafe fn free_steps(steps: StepArray) {
    let steps = steps.0;
    let (v, n) = steps.into_raw_parts();
    for i in 0..n {
        mkr_step_clear(v.add(i));
    }
    libc_free(v as *mut c_void);
}

/// Free a built-but-unattached predicate array.
pub(crate) unsafe fn free_preds(preds: NodeArray) {
    let preds = preds.0;
    let (v, n) = preds.into_raw_parts();
    for i in 0..n {
        mkr_node_free(*v.add(i));
    }
    libc_free(v as *mut c_void);
}

/// A growable array of `T` in the C allocator, so `mkr_step_clear` /
/// `mkr_node_free` can own the result.
struct CArray<T> {
    v: *mut T,
    n: usize,
    cap: usize,
}

impl<T> CArray<T> {
    const fn new() -> CArray<T> {
        CArray {
            v: core::ptr::null_mut(),
            n: 0,
            cap: 0,
        }
    }

    /// Transfer the C allocation to an AST field or a matching destructor.
    ///
    /// `CArray` deliberately has no `Drop`: the element cleanup depends on
    /// the AST field receiving it (`mkr_step_clear` vs `mkr_node_free`).
    /// Consuming the array makes that ownership transfer explicit at the two
    /// boundaries where raw pointers are unavoidable.
    fn into_raw_parts(self) -> (*mut T, usize) {
        (self.v, self.n)
    }

    /// Append, growing geometrically. `false` on failure, with `*err` set and
    /// the array unchanged.
    unsafe fn push(&mut self, b: &Build, item: T) -> bool {
        if self.n == self.cap {
            let want =
                match crate::falloc::grow_capacity(self.cap, self.n + 1, core::mem::size_of::<T>())
                {
                    Some(w) => w,
                    None => {
                        b.oom();
                        return false;
                    }
                };
            let p =
                mkr_reallocarray(self.v as *mut c_void, want, core::mem::size_of::<T>()) as *mut T;
            if p.is_null() {
                b.oom();
                return false;
            }
            self.v = p;
            self.cap = want;
        }
        core::ptr::write(self.v.add(self.n), item);
        self.n += 1;
        true
    }
}

/// A growable C-owned array of AST steps. Its contents are cleared with
/// `mkr_step_clear` before the allocation is released.
pub(crate) struct StepArray(CArray<Step>);

impl StepArray {
    pub(crate) const fn new() -> Self {
        Self(CArray::new())
    }

    pub(crate) unsafe fn push(&mut self, b: &Build, item: Step) -> bool {
        self.0.push(b, item)
    }

    pub(crate) unsafe fn install_into_path(self, path: *mut Node) {
        let (steps, nsteps) = self.0.into_raw_parts();
        (*path).u.path.steps = steps;
        (*path).u.path.nsteps = nsteps;
    }
}

/// A growable C-owned array of owned AST node pointers. Its contents are
/// recursively freed with `mkr_node_free` before the allocation is released.
pub(crate) struct NodeArray(CArray<*mut Node>);

impl NodeArray {
    pub(crate) const fn new() -> Self {
        Self(CArray::new())
    }

    pub(crate) unsafe fn push(&mut self, b: &Build, item: *mut Node) -> bool {
        self.0.push(b, item)
    }

    pub(crate) unsafe fn install_into_step(self, step: *mut Step) {
        let (predicates, npredicates) = self.0.into_raw_parts();
        (*step).predicates = predicates;
        (*step).npredicates = npredicates;
    }
}
