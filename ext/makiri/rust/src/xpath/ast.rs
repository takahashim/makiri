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

#![forbid(unsafe_code)]

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

/// A step's node test (XPath 1.0 section 2.3), each shape carrying exactly the
/// names it has. A `None` prefix is an omitted one, which is not the same as an
/// empty one: an unprefixed test has no prefix, while a CSS `*|el` or `|el` is
/// lowered to an explicit shape of its own.
///
/// `S` is how a name is stored: the AST owns its names (`Box<[u8]>`, the
/// default), while the compiled test borrows them (`&[u8]`) so the hot match
/// reads them in place. One shape, one definition, so the two cannot drift.
#[derive(Clone, Copy)]
pub enum NodeTest<S = Box<[u8]>> {
    /// `local` or `prefix:local`.
    Name { prefix: Option<S>, local: S },
    /// `*` or `prefix:*`: any node of the axis's principal type.
    Wildcard { prefix: Option<S> },
    /// `node()`.
    Node,
    /// `text()`.
    Text,
    /// `comment()`.
    Comment,
    /// `processing-instruction()`, with its optional target literal.
    Pi(Option<S>),
}

pub struct Step {
    pub axis: Axis,
    pub test: NodeTest,
    pub predicates: Vec<Expr>,
}

impl<S: AsRef<[u8]>> NodeTest<S> {
    /// The test's prefix, for the two shapes that can carry one.
    pub fn prefix(&self) -> Option<&[u8]> {
        match self {
            NodeTest::Name { prefix, .. } | NodeTest::Wildcard { prefix } => {
                prefix.as_ref().map(AsRef::as_ref)
            }
            NodeTest::Node | NodeTest::Text | NodeTest::Comment | NodeTest::Pi(_) => None,
        }
    }
}

impl NodeTest<Box<[u8]>> {
    /// `*`, unprefixed.
    pub const ANY: NodeTest = NodeTest::Wildcard { prefix: None };
}

impl Step {
    /// A step with no predicates.
    pub fn new(axis: Axis, test: NodeTest) -> Step {
        Step {
            axis,
            test,
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
    /// one evaluate. Set by the pass in `ast_ops::finish`.
    pub context_independent: bool,
    /// This subtree's slot in the per-evaluate memo table, when remembering its
    /// value can save work: it is context-independent and may be evaluated
    /// more than once in one evaluate. Assigned by the same pass.
    pub memo: Option<u32>,
    /// Levels from this node down to its deepest leaf, itself included.
    depth: u32,
}

impl Expr {
    /// A node over already-built children. Its depth comes from theirs, so it
    /// is known without walking the subtree; the builders check it against
    /// `limits::MAX_AST_DEPTH`.
    pub fn new(kind: ExprKind) -> Expr {
        fn deepest(exprs: &[Expr]) -> u32 {
            exprs.iter().map(|e| e.depth).max().unwrap_or(0)
        }
        fn deepest_predicate(steps: &[Step]) -> u32 {
            steps
                .iter()
                .map(|s| deepest(&s.predicates))
                .max()
                .unwrap_or(0)
        }
        let below = match &kind {
            ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_) | ExprKind::VarRef { .. } => 0,
            ExprKind::FnCall { args, .. } => deepest(args),
            ExprKind::Negate(x) => x.depth,
            ExprKind::BinOp { lhs, rhs, .. } => lhs.depth.max(rhs.depth),
            ExprKind::Path(p) => deepest_predicate(&p.steps),
            ExprKind::Filter {
                expr,
                predicates,
                steps,
            } => expr
                .depth
                .max(deepest(predicates))
                .max(deepest_predicate(steps)),
        };
        Expr {
            kind,
            context_independent: false,
            memo: None,
            depth: below.saturating_add(1),
        }
    }

    pub fn depth(&self) -> u32 {
        self.depth
    }
}

/// A compiled expression: the root and the size of the memo table an evaluate
/// of it needs.
pub struct Ast {
    root: Expr,
    memo_slots: u32,
    /// How many nodes the parser made - 0 when unknown (a CSS lowering).
    nodes: usize,
}

impl Ast {
    /// An AST whose subtrees are not memoized.
    pub fn new(root: Expr) -> Ast {
        Ast {
            root,
            memo_slots: 0,
            nodes: 0,
        }
    }

    /// An AST whose `root` already carries slots `0..memo_slots`.
    pub(crate) fn with_memo_slots(root: Expr, memo_slots: u32) -> Ast {
        Ast {
            root,
            memo_slots,
            nodes: 0,
        }
    }

    /// This AST, recording that the parser made `nodes` nodes for it.
    pub(crate) fn with_node_count(self, nodes: usize) -> Ast {
        Ast { nodes, ..self }
    }

    /// Roughly the heap this AST holds: one `Expr` per node the parser made.
    /// A floor - a node's own lists and names are not counted - which is the
    /// safe direction for a GC report.
    pub fn heap_estimate(&self) -> usize {
        core::mem::size_of::<Ast>()
            .saturating_add(self.nodes.saturating_mul(core::mem::size_of::<Expr>()))
    }

    pub fn root(&self) -> &Expr {
        &self.root
    }

    pub fn memo_slots(&self) -> usize {
        self.memo_slots as usize
    }
}
