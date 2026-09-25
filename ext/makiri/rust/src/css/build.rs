//! AST builders shared by the lowering.
//!
//! Every one takes the nodes it is handed as [`Built`] and returns one: an `Err`
//! operand fails the build, and the operands it did receive are dropped - freed -
//! on the way out. So a caller chains builders without tracking partial state.
//!
//! Each expression node is charged against the AST budget where the builder
//! makes it, as the parser does, and every allocation goes through `falloc`.

#![forbid(unsafe_code)]

use super::Build;
use crate::falloc::{try_box, try_to_boxed_slice, VecPush};
use crate::text::VerifiedText;
use crate::xpath::ast::{Axis, Expr, ExprKind, Op, Path, Step, TestKind};
use crate::xpath::limits::check_ast_depth;
use crate::xpath::msg::{Reported, Status};
use core::ffi::CStr;

/// A node under construction, or the proof its build failed with `*err` set.
pub(crate) type Built = Result<Expr, Reported>;

/// Charge one expression node against the AST budget.
pub(crate) fn charge(b: &Build) -> Result<(), Reported> {
    b.budget.borrow_mut().charge_ast_node()
}

/// `kind` as a node, refused if it would nest the AST too deeply.
pub(crate) fn expr(b: &Build, kind: ExprKind) -> Built {
    let e = Expr::new(kind);
    check_ast_depth(&e, b.err.clone())?;
    Ok(e)
}

/// An owned copy of `s` for an AST name, or `Err` with `*err` set.
pub(crate) fn copy_text(b: &Build, s: &[u8]) -> Result<Box<[u8]>, Reported> {
    if VerifiedText::from_bytes(s).is_none() {
        return Err(b.fail(Status::Internal, c"invalid internal CSS text"));
    }
    try_to_boxed_slice(s).ok_or_else(|| b.fail(Status::Oom, c"css name"))
}

/// `e` on the heap, for an operand slot.
pub(crate) fn boxed(b: &Build, e: Expr) -> Result<Box<Expr>, Reported> {
    try_box(e).map_err(|_| b.oom())
}

/// Append to a list the AST owns; `item` is dropped if it cannot grow.
pub(crate) fn push<T>(b: &Build, list: &mut Vec<T>, item: T) -> Result<(), Reported> {
    list.falloc_push(item).map_err(|_| b.oom())
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

/// `items` joined left to right by `op` - the comma list's union, the
/// selector-list pseudo-classes' OR. An empty list is refused with `empty`, so
/// every failure is reported even where Lexbor already rejects one.
pub(crate) fn fold(b: &Build, op: Op, items: impl Iterator<Item = Built>, empty: &CStr) -> Built {
    let mut acc: Option<Expr> = None;
    for item in items {
        let item = item?;
        acc = Some(match acc {
            None => item,
            Some(lhs) => binop(b, op, Ok(lhs), Ok(item))?,
        });
    }
    acc.ok_or_else(|| b.fail(Status::Syntax, empty))
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

/// A relative PATH of the one step `step`, predicates and all.
pub(crate) fn single_step_path(b: &Build, step: Step) -> Built {
    let mut steps = Vec::new();
    push(b, &mut steps, step)?;
    path(b, steps)
}

/// A one-step relative PATH with no predicates and a wildcard or kind test:
/// `preceding-sibling::*`, `child::node()`, `self::node()` and the rest.
pub(crate) fn step_path(b: &Build, axis: Axis, kind: TestKind) -> Built {
    single_step_path(b, Step::new(axis, kind))
}

/// A one-step relative PATH with a NAME test: `axis::[prefix:]local`.
///
/// The shared builder for every named single-step path - `@prefix:name`, the
/// typed of-type sibling tests - so the step and its two names exist once.
pub(crate) fn named_step_path(b: &Build, axis: Axis, prefix: Option<&[u8]>, name: &[u8]) -> Built {
    let mut step = Step::new(axis, TestKind::Name);
    step.test.local = Some(copy_text(b, name)?);
    if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
        step.test.prefix = Some(copy_text(b, prefix)?);
    }
    single_step_path(b, step)
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
