//! AST builders shared by the lowering.
//!
//! Every one takes the nodes it is handed as [`Built`] and returns one: an `Err`
//! operand fails the build, and the operands it did receive are dropped - freed -
//! on the way out. So a caller chains builders without tracking partial state,
//! which is what the C's cascade of `if (x == NULL) { free(...); return NULL; }`
//! was doing, spelled once per builder instead of once per call.
//!
//! Each expression node is charged against the AST budget where the builder
//! makes it, as the parser does, and every allocation goes through `falloc`.

#![forbid(unsafe_code)]

use super::Build;
use crate::falloc::{try_box, try_to_boxed_slice, VecPush};
use crate::text::VerifiedText;
use crate::xpath::ast::{Axis, Expr, ExprKind, Op, Path, Step, TestKind};
use crate::xpath::limits::check_ast_depth;
use crate::xpath::msg::Reported;

/// A node under construction, or the proof its build failed with `*err` set.
pub(crate) type Built = Result<Expr, Reported>;

/// Charge one expression node against the AST budget.
pub(crate) fn charge(b: &Build) -> Result<(), Reported> {
    b.budget.borrow_mut().charge_ast_node()
}

/// `kind` as a node, refused if it would nest the AST too deeply.
pub(crate) fn expr(b: &Build, kind: ExprKind) -> Built {
    let e = Expr::new(kind);
    check_ast_depth(&e, b.err)?;
    Ok(e)
}

/// An owned copy of `s` for an AST name, or `Err` with `*err` set.
pub(crate) fn copy_text(b: &Build, s: &[u8]) -> Result<Box<[u8]>, Reported> {
    if VerifiedText::from_bytes(s).is_none() {
        return Err(crate::err_setf!(
            b.err,
            crate::xpath::msg::XP_ERR_INTERNAL,
            "invalid internal CSS text"
        ));
    }
    try_to_boxed_slice(s).ok_or_else(|| b.fail(super::ERR_OOM, c"css name"))
}

/// `e` on the heap, for an operand slot.
pub(crate) fn boxed(b: &Build, e: Expr) -> Result<Box<Expr>, Reported> {
    try_box(e).map_err(|_| b.oom())
}

/// Append to a list the AST owns; `item` is dropped if it cannot grow.
pub(crate) fn push<T>(b: &Build, list: &mut Vec<T>, item: T) -> Result<(), Reported> {
    list.mkr_push(item).map_err(|_| b.oom())
}

pub(crate) fn literal(b: &Build, s: &[u8]) -> Built {
    charge(b)?;
    expr(b, ExprKind::LiteralStr(copy_text(b, s)?))
}

pub(crate) fn num(b: &Build, v: f64) -> Built {
    charge(b)?;
    expr(b, ExprKind::LiteralNum(v))
}

/// `lhs op rhs`. An `Err` operand fails without charging, dropping the other.
pub(crate) fn binop(b: &Build, op: Op, lhs: Built, rhs: Built) -> Built {
    let (lhs, rhs) = (lhs?, rhs?);
    charge(b)?;
    expr(
        b,
        ExprKind::BinOp {
            op,
            lhs: boxed(b, lhs)?,
            rhs: boxed(b, rhs)?,
        },
    )
}

/// A call to an internal, compile-time-known function name. Any `Err` argument
/// fails the call; the arguments already collected are dropped with the list.
pub(crate) fn call<const N: usize>(b: &Build, name: &[u8], args: [Built; N]) -> Built {
    let mut argv = Vec::new();
    for arg in args {
        push(b, &mut argv, arg?)?;
    }
    charge(b)?;
    expr(
        b,
        ExprKind::FnCall {
            prefix: None,
            name: copy_text(b, name)?,
            args: argv,
        },
    )
}

/// A one-argument call, the shape most of the lowering wants.
pub(crate) fn call1(b: &Build, name: &[u8], a0: Built) -> Built {
    call(b, name, [a0])
}

/// A two-argument call.
pub(crate) fn call2(b: &Build, name: &[u8], a0: Built, a1: Built) -> Built {
    call(b, name, [a0, a1])
}

/// A relative PATH node over already-built steps.
pub(crate) fn path(b: &Build, steps: Vec<Step>) -> Built {
    charge(b)?;
    expr(
        b,
        ExprKind::Path(Path {
            absolute: false,
            steps,
        }),
    )
}

/// A one-step relative PATH with no predicates: `axis::nodetest`.
///
/// `local` is `None` for a wildcard or a kind test. Used for `@attr`,
/// `preceding-sibling::*`, `child::node()` and the rest.
pub(crate) fn step_path(b: &Build, axis: Axis, nt_kind: TestKind, local: Option<&[u8]>) -> Built {
    named_step_path_inner(b, axis, None, local, nt_kind)
}

/// A one-step relative PATH with a NAME test: `axis::[prefix:]local`.
///
/// The shared builder for every named single-step path - `@prefix:name`, the
/// of-type `not()`, the nth-of-type position - so the step, its two names and
/// the charge exist once.
pub(crate) fn named_step_path(b: &Build, axis: Axis, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    named_step_path_inner(b, axis, prefix, Some(name), TestKind::Name)
}

fn named_step_path_inner(
    b: &Build,
    axis: Axis,
    prefix: Option<&[u8]>,
    local: Option<&[u8]>,
    nt_kind: TestKind,
) -> Built {
    charge(b)?;
    let mut step = Step::new(axis, nt_kind);

    if nt_kind == TestKind::Name {
        if let Some(local) = local {
            step.test.local = Some(copy_text(b, local)?);
        }
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            step.test.prefix = Some(copy_text(b, prefix)?);
        }
    }

    let mut steps = Vec::new();
    push(b, &mut steps, step)?;
    expr(
        b,
        ExprKind::Path(Path {
            absolute: false,
            steps,
        }),
    )
}

/// `@prefix:name` (or `@name`) as a relative attribute-axis path.
pub(crate) fn attr_ns(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    named_step_path(b, Axis::Attribute, prefix, name)
}

/// `@name` with no namespace.
pub(crate) fn attr(b: &Build, name: &[u8]) -> Built {
    attr_ns(b, None, name)
}

/// `normalize-space(@[prefix:]name)`.
pub(crate) fn norm_attr(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    call1(b, b"normalize-space", attr_ns(b, prefix, name))
}

/// `concat(" ", normalize-space(@name), " ")` - the whitespace-padded token list.
pub(crate) fn padded_tokens(b: &Build, prefix: Option<&[u8]>, name: &[u8]) -> Built {
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
pub(crate) fn token_match(
    b: &Build,
    prefix: Option<&[u8]>,
    attr_name: &[u8],
    value: &[u8],
) -> Built {
    /* The padded literal needs to live only until `literal` copies it into the
     * AST. */
    let Some(mut padded) = crate::falloc::try_vec_with_capacity::<u8>(value.len() + 2) else {
        return Err(b.oom());
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
