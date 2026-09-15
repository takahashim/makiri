//! AST builders shared by the lowering.
//!
//! Every one takes the nodes it is handed as [`Built`] and returns one: a `None`
//! operand fails the build, and the operands it did receive are dropped - freed -
//! on the way out. So a caller chains builders without tracking partial state,
//! which is what the C's cascade of `if (x == NULL) { free(...); return NULL; }`
//! was doing, spelled once per builder instead of once per call.
//!
//! The allocations match the AST destructors exactly: nodes through
//! `mkr_node_alloc`, owned text through `TextSlot::try_copy`, arrays through
//! `xpath::own`'s `NodeArray` / `StepArray`. That is why none of this uses a
//! `Vec`: the destructors walk these C-layout fields and free them with libc.

use super::Build;
pub(crate) use crate::xpath::own::{Ast, NodeArray, OwnedStep, StepArray};
use crate::xpath_abi::{
    mkr_node_alloc, TextSlot, VerifiedText, NK_BINOP, NK_FNCALL, NK_LITERAL_NUM, NK_LITERAL_STR,
    NK_PATH, NT_NAME,
};

/// A node under construction, or `None` once its build has failed with `*err`
/// set.
pub(crate) type Built = Option<Ast>;

#[inline]
fn borrowed(s: &[u8]) -> Option<VerifiedText> {
    VerifiedText::from_bytes(s)
}

/// A zeroed node of `kind`, charged against the AST budget.
pub(crate) unsafe fn node(b: &Build, kind: u32) -> Built {
    Ast::from_raw(mkr_node_alloc(b.limits, b.err, kind))
}

/// Copy `s` into an owned text slot. `false` on failure, with `*err` set.
pub(crate) unsafe fn set_text(b: &Build, out: *mut TextSlot, s: &[u8]) -> bool {
    let Some(text) = borrowed(s) else {
        crate::err_setf!(
            b.err,
            crate::xpath_abi::XP_ERR_INTERNAL,
            "invalid internal CSS text"
        );
        return false;
    };
    match crate::xpath_abi::TextSlot::try_copy(text.into(), b.err, Some(c"css name")) {
        Some(value) => {
            *out = value;
            true
        }
        None => false,
    }
}

pub(crate) unsafe fn literal(b: &Build, s: &[u8]) -> Built {
    let mut n = node(b, NK_LITERAL_STR)?;
    if !set_text(b, &mut n.node_mut().u.literal, s) {
        return None;
    }
    Some(n)
}

pub(crate) unsafe fn num(b: &Build, v: f64) -> Built {
    let mut n = node(b, NK_LITERAL_NUM)?;
    n.node_mut().u.literal_num = v;
    Some(n)
}

/// `lhs op rhs`. A `None` operand fails without allocating, dropping the other.
pub(crate) unsafe fn binop(b: &Build, op: u32, lhs: Built, rhs: Built) -> Built {
    let (lhs, rhs) = (lhs?, rhs?);
    let mut n = node(b, NK_BINOP)?;
    let bin = &mut n.node_mut().u.binop;
    bin.op = op;
    bin.lhs = lhs.into_raw();
    bin.rhs = rhs.into_raw();
    Some(n)
}

/// A call to an internal, compile-time-known function name. Any `None` argument
/// fails the call; the arguments already collected are dropped with the array.
pub(crate) unsafe fn call<const N: usize>(b: &Build, name: &[u8], args: [Built; N]) -> Built {
    let mut argv = NodeArray::new();
    for arg in args {
        if argv.try_push(arg?).is_err() {
            b.oom();
            return None;
        }
    }
    let mut n = node(b, NK_FNCALL)?;
    if !set_text(b, &mut n.node_mut().u.fncall.name, name) {
        return None;
    }
    argv.install_as_args(n.as_raw());
    Some(n)
}

/// A one-argument call, the shape most of the lowering wants.
pub(crate) unsafe fn call1(b: &Build, name: &[u8], a0: Built) -> Built {
    call(b, name, [a0])
}

/// A two-argument call.
pub(crate) unsafe fn call2(b: &Build, name: &[u8], a0: Built, a1: Built) -> Built {
    call(b, name, [a0, a1])
}

/// A relative PATH node over an already-built step array.
pub(crate) unsafe fn path(b: &Build, steps: StepArray) -> Built {
    let mut n = node(b, NK_PATH)?;
    n.node_mut().u.path.absolute = 0;
    steps.install_into_path(n.as_raw());
    Some(n)
}

/// A one-step relative PATH with no predicates: `axis::nodetest`.
///
/// `local` is `None` for a wildcard or a kind test. Used for `@attr`,
/// `preceding-sibling::*`, `child::node()` and the rest.
pub(crate) unsafe fn step_path(b: &Build, axis: u32, nt_kind: u32, local: Option<&[u8]>) -> Built {
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
) -> Built {
    named_step_path_inner(b, axis, prefix, Some(name), NT_NAME)
}

unsafe fn named_step_path_inner(
    b: &Build,
    axis: u32,
    prefix: Option<&[u8]>,
    local: Option<&[u8]>,
    nt_kind: u32,
) -> Built {
    let mut n = node(b, NK_PATH)?;
    let mut step = OwnedStep::new(axis, nt_kind);

    if nt_kind == NT_NAME {
        if let Some(local) = local {
            if !set_text(b, &mut step.test.local, local) {
                return None;
            }
        }
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            if !set_text(b, &mut step.test.prefix, prefix) {
                return None;
            }
        }
    }

    let mut steps = StepArray::new();
    if steps.try_push(step).is_err() {
        b.oom();
        return None;
    }
    n.node_mut().u.path.absolute = 0;
    steps.install_into_path(n.as_raw());
    Some(n)
}

/// `@prefix:name` (or `@name`) as a relative attribute-axis path.
pub(crate) unsafe fn attr_ns(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    named_step_path(b, crate::xpath_abi::AXIS_ATTRIBUTE, prefix, name)
}

/// `@name` with no namespace.
pub(crate) unsafe fn attr(b: &Build, name: &[u8]) -> Built {
    attr_ns(b, None, name)
}

/// `normalize-space(@[prefix:]name)`.
pub(crate) unsafe fn norm_attr(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    call1(b, b"normalize-space", attr_ns(b, prefix, name))
}

/// `concat(" ", normalize-space(@name), " ")` - the whitespace-padded token list.
pub(crate) unsafe fn padded_tokens(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    call(
        b,
        b"concat",
        [
            literal(b, b" "),
            norm_attr(b, prefix, name),
            literal(b, b" "),
        ],
    )
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
) -> Built {
    /* The padded literal is built on the Rust stack rather than in a C
     * allocation: it is copied into the AST by `literal`, so it needs to live
     * only until then. The C malloc'd it because it had no other way to
     * concatenate. */
    let Some(mut padded) = crate::falloc::try_vec_with_capacity::<u8>(value.len() + 2) else {
        b.oom();
        return None;
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
