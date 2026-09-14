//! AST builders shared by the lowering.
//!
//! Every one takes ownership of the nodes it is handed and frees them on any
//! failure, so a caller can chain them without tracking partial state - which is
//! what the C's cascade of `if (x == NULL) { free(...); return NULL; }` was
//! doing, spelled once per builder instead of once per call.
//!
//! The allocations match the AST owner's contract exactly: nodes through
//! `mkr_node_alloc`, owned text through `OwnedText::try_copy`, arrays through
//! the C allocator. That is why none of this uses `falloc` or a
//! `Vec` - the AST owner eventually walks these C-layout fields.

use core::ffi::c_void;

use super::Build;
use crate::falloc::raw::mkr_reallocarray;
use crate::xpath::own::Ast;
use crate::xpath_abi::{
    mkr_node_alloc, Node, OwnedText, Step, VerifiedText, NK_BINOP, NK_FNCALL, NK_LITERAL_NUM,
    NK_LITERAL_STR, NK_PATH, NT_NAME,
};

use crate::falloc::raw::mkr_callocarray;
pub use crate::xpath::ast_ops::mkr_step_clear;

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
    match crate::xpath_abi::OwnedText::try_copy(borrowed(s), b.err, c"css name".as_ptr()) {
        Some(value) => {
            *out = value;
            true
        }
        None => false,
    }
}

pub(crate) unsafe fn literal(b: &Build, s: &[u8]) -> *mut Node {
    let n = node(b, NK_LITERAL_STR);
    if n.is_null() {
        return n;
    }
    if !set_text(b, &mut (*n).u.literal, s) {
        Ast::drop_raw(n);
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
        Ast::drop_raw(lhs);
        Ast::drop_raw(rhs);
        return core::ptr::null_mut();
    }
    let n = node(b, NK_BINOP);
    if n.is_null() {
        Ast::drop_raw(lhs);
        Ast::drop_raw(rhs);
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
pub(crate) unsafe fn args(b: &Build, n: usize) -> Option<NodeArray> {
    NodeArray::with_slots(b, n)
}

/// A call to an internal, compile-time-known function name, taking ownership of
/// `argv[0..nargs)`. Frees them on any failure.
pub(crate) unsafe fn fncall(b: &Build, name: &[u8], argv: NodeArray) -> *mut Node {
    let nargs = argv.len();
    for i in 0..nargs {
        if argv.get(i).is_null() {
            drop(argv);
            return core::ptr::null_mut();
        }
    }
    let n = node(b, NK_FNCALL);
    if n.is_null() {
        drop(argv);
        return core::ptr::null_mut();
    }
    if !set_text(b, &mut (*n).u.fncall.name, name) {
        drop(argv);
        Ast::drop_raw(n);
        return core::ptr::null_mut();
    }
    (*n).u.fncall.args = argv.into_raw_parts().0;
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
    let mut step: Step = core::mem::zeroed();
    step.axis = axis;
    step.test.kind = nt_kind;

    if nt_kind == NT_NAME {
        if let Some(local) = local {
            if !set_text(b, &mut step.test.local, local) {
                mkr_step_clear(&mut step);
                Ast::drop_raw(n);
                return core::ptr::null_mut();
            }
        }
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            if !set_text(b, &mut step.test.prefix, prefix) {
                mkr_step_clear(&mut step);
                Ast::drop_raw(n);
                return core::ptr::null_mut();
            }
        }
    }

    let mut steps = StepArray::new();
    if let Err(mut step) = steps.push(b, step) {
        mkr_step_clear(&mut step);
        Ast::drop_raw(n);
        return core::ptr::null_mut();
    }
    (*n).u.path.absolute = 0;
    steps.install_into_path(n);
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
    let Some(mut a) = args(b, 1) else {
        Ast::drop_raw(a0);
        return core::ptr::null_mut();
    };
    a.set(0, a0);
    fncall(b, name, a)
}

/// A two-argument call.
pub(crate) unsafe fn call2(b: &Build, name: &[u8], a0: *mut Node, a1: *mut Node) -> *mut Node {
    let Some(mut a) = args(b, 2) else {
        Ast::drop_raw(a0);
        Ast::drop_raw(a1);
        return core::ptr::null_mut();
    };
    a.set(0, a0);
    a.set(1, a1);
    fncall(b, name, a)
}

/// `normalize-space(@[prefix:]name)`.
pub(crate) unsafe fn norm_attr(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> *mut Node {
    call1(b, b"normalize-space", attr_ns(b, prefix, name))
}

/// `concat(" ", normalize-space(@name), " ")` - the whitespace-padded token list.
pub(crate) unsafe fn padded_tokens(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> *mut Node {
    let Some(mut a) = args(b, 3) else {
        return core::ptr::null_mut();
    };
    a.set(0, literal(b, b" "));
    a.set(1, norm_attr(b, prefix, name));
    a.set(2, literal(b, b" "));
    fncall(b, b"concat", a)
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
    unsafe fn push(&mut self, b: &Build, item: T) -> Result<(), T> {
        if self.n == self.cap {
            let want =
                match crate::falloc::grow_capacity(self.cap, self.n + 1, core::mem::size_of::<T>())
                {
                    Some(w) => w,
                    None => {
                        b.oom();
                        return Err(item);
                    }
                };
            let p =
                mkr_reallocarray(self.v as *mut c_void, want, core::mem::size_of::<T>()) as *mut T;
            if p.is_null() {
                b.oom();
                return Err(item);
            }
            self.v = p;
            self.cap = want;
        }
        core::ptr::write(self.v.add(self.n), item);
        self.n += 1;
        Ok(())
    }
}

/// A growable C-owned array of AST steps. Its contents are cleared with
/// `mkr_step_clear` before the allocation is released.
pub(crate) struct StepArray(CArray<Step>);

impl Drop for StepArray {
    fn drop(&mut self) {
        // SAFETY: every initialized entry is a zeroed or fully-owned `Step`.
        unsafe {
            for i in 0..self.0.n {
                mkr_step_clear(self.0.v.add(i));
            }
            libc_free(self.0.v as *mut c_void);
        }
    }
}

impl StepArray {
    pub(crate) const fn new() -> Self {
        Self(CArray::new())
    }

    /// Append `item`, returning it unchanged when allocation fails.
    pub(crate) unsafe fn push(&mut self, b: &Build, item: Step) -> Result<(), Step> {
        self.0.push(b, item)
    }

    pub(crate) unsafe fn install_into_path(self, path: *mut Node) {
        let (steps, nsteps) = self.into_raw_parts();
        (*path).u.path.steps = steps;
        (*path).u.path.nsteps = nsteps;
    }

    /// Transfer the allocation to the C-layout path field without running
    /// `Drop` for this array.
    unsafe fn into_raw_parts(self) -> (*mut Step, usize) {
        let this = core::mem::ManuallyDrop::new(self);
        core::ptr::read(&this.0).into_raw_parts()
    }
}

/// A growable C-owned array of owned AST node pointers. Its contents are
/// recursively freed with `mkr_node_free` before the allocation is released.
pub(crate) struct NodeArray(CArray<*mut Node>);

impl Drop for NodeArray {
    fn drop(&mut self) {
        // SAFETY: entries are either null (calloc/unused slots) or owned ASTs.
        unsafe {
            for i in 0..self.0.n {
                Ast::drop_raw(*self.0.v.add(i));
            }
            libc_free(self.0.v as *mut c_void);
        }
    }
}

impl NodeArray {
    pub(crate) const fn new() -> Self {
        Self(CArray::new())
    }

    pub(crate) unsafe fn with_slots(b: &Build, n: usize) -> Option<Self> {
        let capacity = if n == 0 { 1 } else { n };
        let v = mkr_callocarray(capacity, core::mem::size_of::<*mut Node>()) as *mut *mut Node;
        if v.is_null() {
            b.oom();
            return None;
        }
        Some(Self(CArray {
            v,
            n,
            cap: capacity,
        }))
    }

    /// Create a one-element predicate array, taking ownership of `item` on
    /// success. The caller retains ownership when allocation fails.
    pub(crate) unsafe fn single(b: &Build, item: *mut Node) -> Option<Self> {
        let mut array = Self::new();
        if array.push(b, item) {
            Some(array)
        } else {
            None
        }
    }

    pub(crate) unsafe fn push(&mut self, b: &Build, item: *mut Node) -> bool {
        self.0.push(b, item).is_ok()
    }

    pub(crate) unsafe fn set(&mut self, index: usize, item: *mut Node) {
        debug_assert!(index < self.0.n);
        *self.0.v.add(index) = item;
    }

    unsafe fn get(&self, index: usize) -> *mut Node {
        debug_assert!(index < self.0.n);
        *self.0.v.add(index)
    }

    fn len(&self) -> usize {
        self.0.n
    }

    pub(crate) unsafe fn install_into_step(self, step: *mut Step) {
        let (predicates, npredicates) = self.into_raw_parts();
        (*step).predicates = predicates;
        (*step).npredicates = npredicates;
    }

    /// Transfer the allocation to the C-layout step field without running
    /// `Drop` for this array.
    unsafe fn into_raw_parts(self) -> (*mut *mut Node, usize) {
        let this = core::mem::ManuallyDrop::new(self);
        core::ptr::read(&this.0).into_raw_parts()
    }
}
