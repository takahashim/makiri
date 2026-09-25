//! XPath 1.0 recursive-descent parser.
//!
//! It builds the same AST as the CSS lowering, so the evaluator runs what either
//! produces. A part under construction is ordinary owned data - a boxed operand,
//! a step, a list - so an error path simply returns and the drops free what was
//! built.
//!
//! Each expression node is charged against the AST budget where the C allocated
//! it, so a query over the limit fails at the same point, with the same error, as
//! it always has. Each is also held to `limits::MAX_AST_DEPTH` as it is made.
//!
//! Lookahead is one token, except where the grammar needs two (a NAME that may
//! be an axis, a node-type keyword, or a function name), which is done by
//! advancing and keeping the token that was there.

#![forbid(unsafe_code)]

use super::abi::*;
use super::ast_ops;
use super::lex::{LexErr, Lexer, Tok, Token};
use super::limits::check_ast_depth;
use super::msg::Bytes;
use crate::err_setf;
use crate::falloc::{try_box, try_to_boxed_slice, VecPush};

struct Parser<'a> {
    lx: Lexer<'a>,
    err: ErrSink,
    budget: &'a mut Budget,
}

/// A parse step: the value, or proof its error was written to the budget.
type PResult<T = ()> = Result<T, Reported>;

/// A name's optional prefix and its local part.
type QualifiedName = (Option<Box<[u8]>>, Box<[u8]>);

/// Report a lexer failure as the run's error. A free function because
/// the very first token is lexed before there is a parser to hold it.
fn lex_err(err: ErrSink, e: LexErr) -> Reported {
    match e {
        LexErr::ExpectedNumber => err_setf!(err, XP_ERR_SYNTAX, "expected number"),
        LexErr::UnterminatedString => {
            err_setf!(err, XP_ERR_SYNTAX, "unterminated string literal")
        }
        LexErr::InvalidUtf8Literal => {
            err_setf!(err, XP_ERR_SYNTAX, "invalid UTF-8 in string literal")
        }
        LexErr::UnexpectedChar(c) => {
            if (0x20..0x7F).contains(&c) {
                err_setf!(err, XP_ERR_SYNTAX, "unexpected character '{}'", c as char)
            } else {
                err_setf!(err, XP_ERR_SYNTAX, "unexpected byte 0x{:02x}", c)
            }
        }
    }
}

/* ---- axis and node-type keywords ---- */

fn axis_by_name(s: &[u8]) -> Option<Axis> {
    Some(match s {
        b"child" => Axis::Child,
        b"descendant" => Axis::Descendant,
        b"parent" => Axis::Parent,
        b"ancestor" => Axis::Ancestor,
        b"following-sibling" => Axis::FollowingSibling,
        b"preceding-sibling" => Axis::PrecedingSibling,
        b"following" => Axis::Following,
        b"preceding" => Axis::Preceding,
        b"attribute" => Axis::Attribute,
        b"namespace" => Axis::Namespace,
        b"self" => Axis::SelfAxis,
        b"descendant-or-self" => Axis::DescendantOrSelf,
        b"ancestor-or-self" => Axis::AncestorOrSelf,
        _ => return None,
    })
}

/// Split a QName at its first ':'.
///
/// A QNAME token always carries one - the lexer sets that kind only in the
/// branch that consumes a ':' - but that invariant lives two modules away and is
/// invisible here, so this answers "what if there is no colon" once rather than
/// per call site. Getting it wrong is not a wrong result: a slice index past the
/// end aborts the process, since a panic cannot become a `Makiri::Error`.
fn split_qname(s: &[u8]) -> (&[u8], &[u8]) {
    match s.iter().position(|&b| b == b':') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, b""),
    }
}

fn is_nodetype_name(s: &[u8]) -> bool {
    matches!(
        s,
        b"node" | b"text" | b"comment" | b"processing-instruction"
    )
}

impl<'a> Parser<'a> {
    fn tok(&self) -> Token {
        self.lx.tok
    }

    fn kind(&self) -> Tok {
        self.lx.tok.kind
    }

    fn text(&self, t: &Token) -> &'a [u8] {
        t.text(self.lx.src())
    }

    fn advance(&mut self) -> PResult {
        let err = self.err.clone();
        self.lx.advance().map_err(|e| lex_err(err, e))
    }

    fn eat(&mut self, k: Tok, what: &str) -> PResult {
        if self.kind() != k {
            return Err(err_setf!(self.err, XP_ERR_SYNTAX, "expected {}", what));
        }
        self.advance()
    }

    /// Charge one expression node against the AST budget.
    fn charge(&mut self) -> PResult {
        self.budget.charge_ast_node()
    }

    /// Copy `text` into an AST name. A failure must be propagated: a missing
    /// name would silently mis-compare at evaluation, so the parse fails closed
    /// instead.
    fn fill_owned(&self, text: &[u8]) -> PResult<Box<[u8]>> {
        try_to_boxed_slice(text)
            .ok_or_else(|| err_setf!(self.err, XP_ERR_OOM, "out of memory in parser"))
    }

    /// `kind` as a node, refused if it would nest the AST too deeply.
    fn node(&self, kind: ExprKind) -> PResult<Expr> {
        let e = Expr::new(kind);
        check_ast_depth(&e, self.err.clone())?;
        Ok(e)
    }

    /// `e` on the heap, for an operand slot.
    fn boxed(&self, e: Expr) -> PResult<Box<Expr>> {
        try_box(e).map_err(|_| err_setf!(self.err, XP_ERR_OOM, "out of memory allocating AST node"))
    }

    /// A name token's optional prefix and local part, copied.
    fn names(&self, t: &Token) -> PResult<QualifiedName> {
        if t.kind == Tok::QName {
            let (p, l) = split_qname(self.text(t));
            Ok((Some(self.fill_owned(p)?), self.fill_owned(l)?))
        } else {
            Ok((None, self.fill_owned(self.text(t))?))
        }
    }

    /// Charge the step budget, then append. A step that does not land is freed.
    fn push_step(&mut self, steps: &mut Vec<Step>, s: Step) -> PResult {
        self.budget.check_steps(steps.len() + 1)?;
        if steps.falloc_push(s).is_err() {
            return Err(err_setf!(
                self.err.clone(),
                XP_ERR_OOM,
                "out of memory growing step array"
            ));
        }
        Ok(())
    }

    /// Parse a run of `('/' | '//') Step`, expanding each `//` into an implicit
    /// `descendant-or-self::node()` step. A non-separator token makes this a
    /// no-op. On failure the caller drops `steps`, freeing what was pushed.
    fn parse_step_tail(&mut self, steps: &mut Vec<Step>) -> PResult {
        while self.kind() == Tok::Slash || self.kind() == Tok::DSlash {
            let dslash = self.kind() == Tok::DSlash;
            self.advance()?;
            if dslash {
                self.push_step(steps, Step::new(Axis::DescendantOrSelf, TestKind::Node))?;
            }
            let next = self.parse_step()?;
            self.push_step(steps, next)?;
        }
        Ok(())
    }

    /* ---- node tests ---- */

    /// `saved` is an already-consumed NAME; the current token is the one after
    /// it. A node-type keyword immediately followed by '(' is a node-type test
    /// (with an optional PI target literal); anything else is an NCName test.
    fn parse_nodetype_or_name(&mut self, saved: Token, out: &mut NodeTest) -> PResult {
        let name = self.text(&saved);
        if is_nodetype_name(name) && self.kind() == Tok::LParen {
            self.advance()?;
            match name {
                b"node" => out.kind = TestKind::Node,
                b"text" => out.kind = TestKind::Text,
                b"comment" => out.kind = TestKind::Comment,
                _ => {
                    out.kind = TestKind::Pi;
                    if self.kind() == Tok::Literal {
                        let t = self.tok();
                        out.pi_target = Some(self.fill_owned(self.text(&t))?);
                        self.advance()?;
                    }
                }
            }
            return self.eat(Tok::RParen, "')' after node type test");
        }
        out.kind = TestKind::Name;
        out.local = Some(self.fill_owned(name)?);
        Ok(())
    }

    /// Called with the current token at the first token of the node test; leaves
    /// it at the token after. `out` is a fresh test with no names yet.
    fn parse_node_test(&mut self, out: &mut NodeTest) -> PResult {
        if self.kind() == Tok::Star {
            out.kind = TestKind::Wildcard;
            return self.advance();
        }
        if self.kind() == Tok::Name {
            let saved = self.tok();
            self.advance()?;
            return self.parse_nodetype_or_name(saved, out);
        }
        if self.kind() == Tok::QName {
            /* `prefix:local` or `prefix:*`. */
            let t = self.tok();
            let (prefix, local) = split_qname(self.text(&t));
            out.prefix = Some(self.fill_owned(prefix)?);
            if local == b"*" {
                out.kind = TestKind::Wildcard;
            } else {
                out.kind = TestKind::Name;
                out.local = Some(self.fill_owned(local)?);
            }
            return self.advance();
        }
        Err(err_setf!(self.err, XP_ERR_SYNTAX, "expected node test"))
    }

    /* ---- predicates ---- */

    fn parse_predicates(&mut self, preds: &mut Vec<Expr>) -> PResult {
        while self.kind() == Tok::LBracket {
            self.budget.check_predicates(preds.len() + 1)?;
            self.advance()?;
            let e = self.parse_expr()?;
            self.eat(Tok::RBracket, "']' to close predicate")?;
            if preds.falloc_push(e).is_err() {
                return Err(err_setf!(
                    self.err,
                    XP_ERR_OOM,
                    "out of memory growing predicate array"
                ));
            }
        }
        Ok(())
    }

    /* ---- steps ---- */

    /// Parse one step. A failure partway leaves an owned name or predicates
    /// behind in the step, which is freed as the error returns.
    fn parse_step(&mut self) -> PResult<Step> {
        let mut step = Step::new(Axis::Child, TestKind::Name);
        self.parse_step_inner(&mut step)?;
        Ok(step)
    }

    fn parse_step_inner(&mut self, out: &mut Step) -> PResult {
        /* Abbreviated steps. */
        if self.kind() == Tok::Dot {
            self.advance()?;
            out.axis = Axis::SelfAxis;
            out.test.kind = TestKind::Node;
            return Ok(());
        }
        if self.kind() == Tok::DotDot {
            self.advance()?;
            out.axis = Axis::Parent;
            out.test.kind = TestKind::Node;
            return Ok(());
        }

        /* AxisSpecifier: '@' or NAME '::'. */
        if self.kind() == Tok::At {
            out.axis = Axis::Attribute;
            self.advance()?;
        } else if self.kind() == Tok::Name {
            /* Axis or NameTest - decided by the token after, so peek by
             * advancing and keeping the NAME. */
            let saved = self.tok();
            self.advance()?;
            if self.kind() == Tok::ColonColon {
                let name = self.text(&saved);
                match axis_by_name(name) {
                    Some(ax) => out.axis = ax,
                    None => {
                        return Err(err_setf!(
                            self.err,
                            XP_ERR_SYNTAX,
                            "unknown axis '{}'",
                            Bytes(name)
                        ));
                    }
                }
                self.advance()?; /* eat '::' */
            } else {
                /* It was a NameTest, and the NAME is already consumed, so replay
                 * it through the shared node-type grammar. */
                out.axis = Axis::Child;
                self.parse_nodetype_or_name(saved, &mut out.test)?;
                return self.parse_predicates(&mut out.predicates);
            }
        } else {
            out.axis = Axis::Child;
        }

        self.parse_node_test(&mut out.test)?;
        self.parse_predicates(&mut out.predicates)
    }

    /* ---- location paths ---- */

    fn parse_relative_path(&mut self, steps: &mut Vec<Step>) -> PResult {
        let s = self.parse_step()?;
        self.push_step(steps, s)?;
        self.parse_step_tail(steps)
    }

    fn can_start_step(&self) -> bool {
        matches!(
            self.kind(),
            Tok::Dot | Tok::DotDot | Tok::At | Tok::Star | Tok::Name | Tok::QName
        )
    }

    fn parse_location_path(&mut self) -> PResult<Expr> {
        self.charge()?;
        let mut steps = Vec::new();
        let absolute = match self.kind() {
            Tok::Slash => {
                self.advance()?;
                if self.can_start_step() {
                    self.parse_relative_path(&mut steps)?;
                }
                true
            }
            Tok::DSlash => {
                /* '//' = '/descendant-or-self::node()/'. Leave the token where it
                 * is so the shared loop expands it itself. */
                self.parse_step_tail(&mut steps)?;
                true
            }
            _ => {
                self.parse_relative_path(&mut steps)?;
                false
            }
        };
        self.node(ExprKind::Path(Path { absolute, steps }))
    }

    /* ---- primaries, function calls, filters ---- */

    fn parse_function_call(&mut self, name_tok: Token) -> PResult<Expr> {
        if self.kind() != Tok::LParen {
            return Err(err_setf!(
                self.err,
                XP_ERR_SYNTAX,
                "expected '(' in function call"
            ));
        }
        self.advance()?;
        self.charge()?;
        let (prefix, name) = self.names(&name_tok)?;

        let mut args = Vec::new();
        if self.kind() != Tok::RParen {
            loop {
                self.budget.check_func_args(args.len() + 1)?;
                let arg = self.parse_expr()?;
                if args.falloc_push(arg).is_err() {
                    return Err(err_setf!(
                        self.err,
                        XP_ERR_OOM,
                        "out of memory growing function argument array"
                    ));
                }
                if self.kind() != Tok::Comma {
                    break;
                }
                self.advance()?;
            }
        }
        self.eat(Tok::RParen, "')' after function arguments")?;
        self.node(ExprKind::FnCall { prefix, name, args })
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        match self.kind() {
            Tok::Dollar => {
                self.advance()?;
                if self.kind() != Tok::Name && self.kind() != Tok::QName {
                    return Err(err_setf!(
                        self.err,
                        XP_ERR_SYNTAX,
                        "expected name after '$'"
                    ));
                }
                self.charge()?;
                let (prefix, name) = self.names(&self.tok())?;
                self.advance()?;
                self.node(ExprKind::VarRef { prefix, name })
            }
            Tok::LParen => {
                self.advance()?;
                let n = self.parse_expr()?;
                self.eat(Tok::RParen, "')' after parenthesised expr")?;
                Ok(n)
            }
            Tok::Literal => {
                self.charge()?;
                let t = self.tok();
                let text = self.fill_owned(self.text(&t))?;
                self.advance()?;
                self.node(ExprKind::LiteralStr(text))
            }
            Tok::Number => {
                self.charge()?;
                let num = self.tok().num;
                self.advance()?;
                self.node(ExprKind::LiteralNum(num))
            }
            Tok::Name | Tok::QName => {
                let name_tok = self.tok();
                self.advance()?;
                if self.kind() == Tok::LParen {
                    return self.parse_function_call(name_tok);
                }
                Err(err_setf!(
                    self.err,
                    XP_ERR_SYNTAX,
                    "expected '(' after function name"
                ))
            }
            _ => Err(err_setf!(
                self.err,
                XP_ERR_SYNTAX,
                "expected primary expression"
            )),
        }
    }

    fn parse_filter_expr(&mut self) -> PResult<Expr> {
        let primary = self.parse_primary()?;
        if !matches!(self.kind(), Tok::LBracket | Tok::Slash | Tok::DSlash) {
            return Ok(primary);
        }
        self.charge()?;
        let expr = self.boxed(primary)?;
        let mut predicates = Vec::new();
        self.parse_predicates(&mut predicates)?;
        /* Optional trailing location path (`$x/foo`, `(expr)//bar`). The shared
         * loop is a no-op when no separator follows. */
        let mut steps = Vec::new();
        self.parse_step_tail(&mut steps)?;
        self.node(ExprKind::Filter {
            expr,
            predicates,
            steps,
        })
    }

    /// LocationPath or FilterExpr?
    fn looks_like_filter_expr(&self) -> bool {
        match self.kind() {
            Tok::Dollar | Tok::LParen | Tok::Literal | Tok::Number => true,
            Tok::Name | Tok::QName => {
                let t = self.tok();
                if t.kind == Tok::Name && is_nodetype_name(self.text(&t)) {
                    return false;
                }
                /* A function call iff '(' follows - peek the byte rather than
                 * running the lexer. */
                self.lx.peek_nonws() == Some(b'(')
            }
            _ => false,
        }
    }

    fn parse_path_expr(&mut self) -> PResult<Expr> {
        if self.kind() == Tok::Slash || self.kind() == Tok::DSlash {
            return self.parse_location_path();
        }
        if self.looks_like_filter_expr() {
            return self.parse_filter_expr();
        }
        self.parse_location_path()
    }

    /* ---- the operator ladder ---- */

    /// `lhs op rhs`, owning both; they are freed if the node cannot be made.
    fn make_binop(&mut self, op: Op, lhs: Expr, rhs: Expr) -> PResult<Expr> {
        self.charge()?;
        self.node(ExprKind::BinOp {
            op,
            lhs: self.boxed(lhs)?,
            rhs: self.boxed(rhs)?,
        })
    }

    fn parse_union(&mut self) -> PResult<Expr> {
        let mut l = self.parse_path_expr()?;
        while self.kind() == Tok::Pipe {
            self.advance()?;
            let r = self.parse_path_expr()?;
            l = self.make_binop(Op::Union, l, r)?;
        }
        Ok(l)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let mut neg = false;
        while self.kind() == Tok::Minus {
            neg = !neg;
            self.advance()?;
        }
        let e = self.parse_union()?;
        if !neg {
            return Ok(e);
        }
        self.charge()?;
        self.node(ExprKind::Negate(self.boxed(e)?))
    }

    /// The binary operator at the current token and its level in
    /// [`BINOP_LEVELS`], if there is one.
    fn binop_here(&self) -> Option<(Op, usize)> {
        BINOP_LEVELS.iter().enumerate().find_map(|(li, level)| {
            level.iter().find_map(|m| {
                let hit = match m.trigger {
                    Trigger::Word(w) => self.lx.tok_is_word(w),
                    Trigger::Kind(k) => self.kind() == k,
                };
                hit.then_some((m.op, li))
            })
        })
    }

    /// Parse an expression whose operators bind no looser than level `max_li`,
    /// left-associatively; the operands are `parse_unary` (prefix '-').
    ///
    /// Precedence climbing rather than one function per level: the tree and
    /// the order nodes are charged in are the same, but reading an operand no
    /// longer descends through every level, which was most of the parser's cost
    /// on a short expression.
    fn parse_binary(&mut self, max_li: usize) -> PResult<Expr> {
        let mut l = self.parse_unary()?;
        while let Some((op, li)) = self.binop_here() {
            if li > max_li {
                break;
            }
            self.advance()?;
            /* The right operand holds only tighter operators, so an operator of
             * this same level after it folds into `l`: left-associative. */
            let r = match li.checked_sub(1) {
                Some(tighter) => self.parse_binary(tighter)?,
                None => self.parse_unary()?,
            };
            l = self.make_binop(op, l, r)?;
        }
        Ok(l)
    }

    fn parse_expr(&mut self) -> PResult<Expr> {
        /* Bound parser recursion so '((((...))))' cannot blow the stack. */
        self.budget.enter_recursion()?;
        let n = self.parse_binary(BINOP_LEVELS.len() - 1);
        self.budget.leave_recursion();
        n
    }
}

/* ---- the precedence table ---- */

/// What an operator is spelled as: a word token ("div", "and", ...) or a
/// token kind.
enum Trigger {
    Word(&'static [u8]),
    Kind(Tok),
}

struct BinMatch {
    trigger: Trigger,
    op: Op,
}

const fn w(word: &'static [u8], op: Op) -> BinMatch {
    BinMatch {
        trigger: Trigger::Word(word),
        op,
    }
}
const fn k(kind: Tok, op: Op) -> BinMatch {
    BinMatch {
        trigger: Trigger::Kind(kind),
        op,
    }
}

/// Tightest-binding level first.
static BINOP_LEVELS: &[&[BinMatch]] = &[
    &[
        k(Tok::Star, Op::Mul),
        w(b"div", Op::Div),
        w(b"mod", Op::Mod),
    ],
    &[k(Tok::Plus, Op::Add), k(Tok::Minus, Op::Sub)],
    &[
        k(Tok::Lt, Op::Lt),
        k(Tok::Gt, Op::Gt),
        k(Tok::Le, Op::Le),
        k(Tok::Ge, Op::Ge),
    ],
    &[k(Tok::Eq, Op::Eq), k(Tok::Ne, Op::Ne)],
    &[w(b"and", Op::And)],
    &[w(b"or", Op::Or)],
];

/* ---- entry ---- */

/// Parse an expression into a compiled AST; `Err` with the budget's error slot
/// filled on failure.
///
/// `expr` is a verified text: NUL-free, valid UTF-8, and its lifetime keeps its
/// bytes live for the parse.
pub fn parse_owned(expr: VerifiedText, budget: &mut Budget) -> Result<Box<Ast>, Reported> {
    let err = budget.sink();
    budget.check_expr_bytes(expr.len())?;

    let src: &[u8] = expr.as_bytes();

    let lx = Lexer::new(src).map_err(|e| lex_err(err.clone(), e))?;
    let mut p = Parser {
        lx,
        err: err.clone(),
        budget,
    };

    let root = p.parse_expr()?;
    if p.kind() != Tok::Eof {
        let t = p.tok();
        return Err(err_setf!(
            err,
            XP_ERR_SYNTAX,
            "trailing input at '{}'",
            Bytes(p.text(&t))
        ));
    }
    try_box(ast_ops::finish(root))
        .map_err(|_| err_setf!(err, XP_ERR_OOM, "out of memory allocating AST node"))
}
