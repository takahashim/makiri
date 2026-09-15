//! The compiled AST: expressions, location steps, node tests, axes and
//! operators, as the parser and the CSS lowering build them and the evaluator
//! walks them.
//!
//! Plain Rust data. Every allocation in it - a boxed operand, a step or
//! predicate list, a name - is made through `falloc`, which is what lets
//! `rake oom` fail each one and the engine raise instead of aborting; freeing
//! is `Drop`.
//!
//! The tree is read-only once built. A context-independent subtree's value is
//! remembered per evaluate in a side table (see [`Expr::memo`]), not in the
//! tree, so one compiled AST can be evaluated re-entrantly - a handler running
//! the same cached expression again - through a shared reference.

/// A location step's axis (XPath 1.0 section 2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Child,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    Name,
    Wildcard,
    Node,
    Text,
    Comment,
    Pi,
}

/// A binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Or,
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

/// A step's node test. `None` is an omitted part, which is not the same as an
/// empty one: an unprefixed test has no prefix, while a CSS `*|el` or `|el` is
/// lowered to an explicit shape of its own.
pub struct NodeTest {
    pub kind: TestKind,
    pub prefix: Option<Box<[u8]>>,
    pub local: Option<Box<[u8]>>,
    pub pi_target: Option<Box<[u8]>>,
}

pub struct Step {
    pub axis: Axis,
    pub test: NodeTest,
    pub predicates: Vec<Expr>,
}

impl Step {
    /// A step with no names and no predicates.
    pub fn new(axis: Axis, kind: TestKind) -> Step {
        Step {
            axis,
            test: NodeTest {
                kind,
                prefix: None,
                local: None,
                pi_target: None,
            },
            predicates: Vec::new(),
        }
    }
}

pub struct Path {
    pub absolute: bool,
    pub steps: Vec<Step>,
}

pub enum ExprKind {
    LiteralStr(Box<[u8]>),
    LiteralNum(f64),
    VarRef {
        prefix: Option<Box<[u8]>>,
        name: Box<[u8]>,
    },
    FnCall {
        prefix: Option<Box<[u8]>>,
        name: Box<[u8]>,
        args: Vec<Expr>,
    },
    Negate(Box<Expr>),
    BinOp {
        op: Op,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Path(Path),
    /// `primary[pred]...` followed by an optional relative path.
    Filter {
        expr: Box<Expr>,
        predicates: Vec<Expr>,
        steps: Vec<Step>,
    },
}

pub struct Expr {
    pub kind: ExprKind,
    /// Whether this subtree evaluates to the same value wherever it appears in
    /// one evaluate. Set by the hoisting pass in `ast_ops`.
    pub context_independent: bool,
    /// This subtree's slot in the per-evaluate memo table, when remembering its
    /// value can save work: it is context-independent and may be evaluated
    /// more than once in one evaluate. Assigned by the same pass.
    pub memo: Option<u32>,
}

impl Expr {
    pub fn new(kind: ExprKind) -> Expr {
        Expr {
            kind,
            context_independent: false,
            memo: None,
        }
    }
}

/// A compiled expression: the root and the size of the memo table an evaluate
/// of it needs.
pub struct Ast {
    root: Expr,
    memo_slots: u32,
}

impl Ast {
    /// An AST whose subtrees are not memoized.
    pub fn new(root: Expr) -> Ast {
        Ast {
            root,
            memo_slots: 0,
        }
    }

    /// An AST whose `root` already carries slots `0..memo_slots`.
    pub(crate) fn with_memo_slots(root: Expr, memo_slots: u32) -> Ast {
        Ast { root, memo_slots }
    }

    pub fn root(&self) -> &Expr {
        &self.root
    }

    pub fn memo_slots(&self) -> usize {
        self.memo_slots as usize
    }
}
