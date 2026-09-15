//! The compiled AST: node kinds, axes, node tests, operators, and the node
//! layout the parser builds and the evaluator walks.
//!
//! The layout is the C engine's. It stays because every node is allocated
//! through `falloc`, which is what lets `rake oom` fail each allocation and the
//! engine raise instead of aborting; ownership is `xpath::own`'s guards.

use core::ffi::c_int;

use super::value::{TextSlot, Val};

/// What an AST node is. Discriminant 0 is a real kind, so a zeroed node is valid.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    LiteralStr = 0,
    LiteralNum,
    VarRef,
    FnCall,
    Unary,
    BinOp,
    Path,
    Filter,
}

/// A location step's axis (XPath 1.0 section 2.2).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Child = 0,
    Descendant,
    Parent,
    Ancestor,
    FollowingSibling,
    PrecedingSibling,
    Following,
    Preceding,
    Attribute,
    Namespace,
    SelfAxis,
    DescendantOrSelf,
    AncestorOrSelf,
}

/// What a node test tests (XPath 1.0 section 2.3).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    Name = 0,
    Wildcard,
    Node,
    Text,
    Comment,
    Pi,
}

/// A binary operator.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Or = 0,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Union,
}

#[derive(Clone, Copy)]
pub struct NodeTest {
    pub kind: TestKind,
    pub prefix: TextSlot,
    pub local: TextSlot,
    pub pi_target: TextSlot,
}

#[derive(Clone, Copy)]
pub struct Step {
    pub axis: Axis,
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
    pub op: Op,
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
/// `node_free`, both in `xpath::ast_ops`.
///
/// The payload union is private and read by kind through [`Node::view`] and
/// [`Node::view_mut`], so no caller picks an arm the kind does not name. The node
/// is calloc'd, so whichever arm is read, its bytes are initialised; `kind` is
/// set once, by `node_alloc`.
pub struct Node {
    pub kind: NodeKind,
    pub is_context_independent: u8,
    pub memoized: u8,
    pub memo_value: Val,
    u: NodeU,
}

/// A node's payload by kind.
#[derive(Clone, Copy)]
pub enum NodeRef<'a> {
    LiteralStr(TextSlot),
    LiteralNum(f64),
    VarRef(&'a VarRef),
    FnCall(&'a FnCall),
    Unary(&'a Unary),
    BinOp(&'a BinOp),
    Path(&'a Path),
    Filter(&'a Filter),
}

/// A node's payload by kind, writable: for the builders, the peephole and the
/// destructor.
pub enum NodeMut<'a> {
    LiteralStr(&'a mut TextSlot),
    LiteralNum(&'a mut f64),
    VarRef(&'a mut VarRef),
    FnCall(&'a mut FnCall),
    Unary(&'a mut Unary),
    BinOp(&'a mut BinOp),
    Path(&'a mut Path),
    Filter(&'a mut Filter),
}

impl Node {
    /// `n`'s payload by kind.
    ///
    /// Takes the node by pointer and borrows only the payload - never the memo
    /// slot beside it, which a nested evaluate (a handler re-entering) may
    /// rewrite while a caller still holds the payload.
    ///
    /// # Safety
    /// `n` must be a live node for `'a`.
    pub unsafe fn view<'a>(n: *const Node) -> NodeRef<'a> {
        match (*n).kind {
            NodeKind::LiteralStr => NodeRef::LiteralStr((*n).u.literal),
            NodeKind::LiteralNum => NodeRef::LiteralNum((*n).u.literal_num),
            NodeKind::VarRef => NodeRef::VarRef(&(*n).u.varref),
            NodeKind::FnCall => NodeRef::FnCall(&(*n).u.fncall),
            NodeKind::Unary => NodeRef::Unary(&(*n).u.unary),
            NodeKind::BinOp => NodeRef::BinOp(&(*n).u.binop),
            NodeKind::Path => NodeRef::Path(&(*n).u.path),
            NodeKind::Filter => NodeRef::Filter(&(*n).u.filter),
        }
    }

    /// [`Node::view`], writable.
    ///
    /// # Safety
    /// `n` must be a live node for `'a` whose payload nothing else uses
    /// meanwhile, and writes must leave it in a state `node_free` can take
    /// apart: an owned child pointer is null or owned by this node alone.
    pub unsafe fn view_mut<'a>(n: *mut Node) -> NodeMut<'a> {
        match (*n).kind {
            NodeKind::LiteralStr => NodeMut::LiteralStr(&mut (*n).u.literal),
            NodeKind::LiteralNum => NodeMut::LiteralNum(&mut (*n).u.literal_num),
            NodeKind::VarRef => NodeMut::VarRef(&mut (*n).u.varref),
            NodeKind::FnCall => NodeMut::FnCall(&mut (*n).u.fncall),
            NodeKind::Unary => NodeMut::Unary(&mut (*n).u.unary),
            NodeKind::BinOp => NodeMut::BinOp(&mut (*n).u.binop),
            NodeKind::Path => NodeMut::Path(&mut (*n).u.path),
            NodeKind::Filter => NodeMut::Filter(&mut (*n).u.filter),
        }
    }
}
