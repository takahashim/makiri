//! XPath 1.0 recursive-descent parser (mkr_xpath_parse.c).
//!
//! It builds the C-layout AST through the same node factory and arrays as the
//! CSS lowering, so the evaluator runs what either produces. Everything under
//! construction is held by a guard - a node by `own::Ast`, a step by
//! `OwnedStep`, an array by `NodeArray` / `StepArray` - and a finished part is
//! installed into its parent's field. An error path simply returns: the drops
//! free what was built.
//!
//! Lookahead is one token, except where the grammar needs two (a NAME that may
//! be an axis, a node-type keyword, or a function name), which is done by
//! advancing and keeping the token that was there.

use super::abi::*;
use super::lex::{LexErr, Lexer, Tok, Token};
use super::msg::Bytes;
use super::own::{Ast, NodeArray, OwnedStep, StepArray};
use crate::err_setf;
use core::ffi::c_int;
use core::ptr;

struct Parser<'a> {
    lx: Lexer<'a>,
    err: ErrSink,
    limits: *mut Limits,
}

/// A parse step: the value, or proof its error was written to `err`.
type PResult<T = ()> = Result<T, Reported>;

/// Report a lexer failure as an `mkr_xpath_error_t`. A free function because
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

fn axis_by_name(s: &[u8]) -> Option<u32> {
    Some(match s {
        b"child" => AXIS_CHILD,
        b"descendant" => AXIS_DESCENDANT,
        b"parent" => AXIS_PARENT,
        b"ancestor" => AXIS_ANCESTOR,
        b"following-sibling" => AXIS_FOLLOWING_SIBLING,
        b"preceding-sibling" => AXIS_PRECEDING_SIBLING,
        b"following" => AXIS_FOLLOWING,
        b"preceding" => AXIS_PRECEDING,
        b"attribute" => AXIS_ATTRIBUTE,
        b"namespace" => AXIS_NAMESPACE,
        b"self" => AXIS_SELF,
        b"descendant-or-self" => AXIS_DESCENDANT_OR_SELF,
        b"ancestor-or-self" => AXIS_ANCESTOR_OR_SELF,
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
        let err = self.err;
        self.lx.advance().map_err(|e| lex_err(err, e))
    }

    fn eat(&mut self, k: Tok, what: &str) -> PResult {
        if self.kind() != k {
            return Err(err_setf!(self.err, XP_ERR_SYNTAX, "expected {}", what));
        }
        self.advance()
    }

    fn new_node(&mut self, kind: u32) -> PResult<Ast> {
        // SAFETY: the parser's limits and error slot are live for the parse.
        unsafe { node_alloc(self.limits, self.err, kind) }
    }

    /// Copy `text` into an owned-text AST slot. A failure must be propagated: a
    /// null slot left in the AST would silently mis-compare at evaluation, so
    /// the parse fails closed instead.
    fn fill_owned(&self, text: &[u8]) -> PResult<TextSlot> {
        // SAFETY: a silent sink is accepted; the parser reports its own.
        unsafe { TextSlot::try_copy_bytes(text, ErrSink::silent(), None) }
            .map_err(|_| err_setf!(self.err, XP_ERR_OOM, "out of memory in parser"))
    }

    /// Charge the step budget, then append. A step that does not land is freed.
    fn push_step(&mut self, steps: &mut StepArray, s: OwnedStep) -> PResult {
        unsafe { limit_check_steps(self.limits, steps.len() + 1, self.err)? };
        if steps.try_push(s).is_err() {
            return Err(err_setf!(
                self.err,
                XP_ERR_OOM,
                "out of memory growing step array"
            ));
        }
        Ok(())
    }

    /// Parse a run of `('/' | '//') Step`, expanding each `//` into an implicit
    /// `descendant-or-self::node()` step. A non-separator token makes this a
    /// no-op. On failure the caller drops `steps`, freeing what was pushed.
    fn parse_step_tail(&mut self, steps: &mut StepArray) -> PResult {
        while self.kind() == Tok::Slash || self.kind() == Tok::DSlash {
            let dslash = self.kind() == Tok::DSlash;
            self.advance()?;
            if dslash {
                self.push_step(steps, OwnedStep::new(AXIS_DESCENDANT_OR_SELF, NT_NODE))?;
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
                b"node" => out.kind = NT_NODE,
                b"text" => out.kind = NT_TEXT,
                b"comment" => out.kind = NT_COMMENT,
                _ => {
                    out.kind = NT_PI;
                    if self.kind() == Tok::Literal {
                        let t = self.tok();
                        let s = self.text(&t);
                        out.pi_target = self.fill_owned(s)?;
                        self.advance()?;
                    }
                }
            }
            return self.eat(Tok::RParen, "')' after node type test");
        }
        out.kind = NT_NAME;
        out.local = self.fill_owned(name)?;
        Ok(())
    }

    /// Called with the current token at the first token of the node test; leaves
    /// it at the token after. `out` is a fresh test with no texts yet.
    fn parse_node_test(&mut self, out: &mut NodeTest) -> PResult {
        if self.kind() == Tok::Star {
            out.kind = NT_WILDCARD;
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
            out.prefix = self.fill_owned(prefix)?;
            if local == b"*" {
                out.kind = NT_WILDCARD;
            } else {
                out.kind = NT_NAME;
                out.local = self.fill_owned(local)?;
            }
            return self.advance();
        }
        Err(err_setf!(self.err, XP_ERR_SYNTAX, "expected node test"))
    }

    /* ---- predicates ---- */

    fn parse_predicates(&mut self, preds: &mut NodeArray) -> PResult {
        while self.kind() == Tok::LBracket {
            unsafe { limit_check_predicates(self.limits, preds.len() + 1, self.err)? };
            self.advance()?;
            let e = self.parse_expr()?;
            self.eat(Tok::RBracket, "']' to close predicate")?;
            if preds.try_push(e).is_err() {
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
    /// behind; the guards free them, so every caller just bails.
    fn parse_step(&mut self) -> PResult<OwnedStep> {
        let mut step = OwnedStep::new(AXIS_CHILD, NT_NAME);
        let mut preds = NodeArray::new();
        self.parse_step_inner(&mut step, &mut preds)?;
        preds.install_into_step(&mut step);
        Ok(step)
    }

    fn parse_step_inner(&mut self, out: &mut Step, preds: &mut NodeArray) -> PResult {
        /* Abbreviated steps. */
        if self.kind() == Tok::Dot {
            self.advance()?;
            out.axis = AXIS_SELF;
            out.test.kind = NT_NODE;
            return Ok(());
        }
        if self.kind() == Tok::DotDot {
            self.advance()?;
            out.axis = AXIS_PARENT;
            out.test.kind = NT_NODE;
            return Ok(());
        }

        /* AxisSpecifier: '@' or NAME '::'. */
        if self.kind() == Tok::At {
            out.axis = AXIS_ATTRIBUTE;
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
                out.axis = AXIS_CHILD;
                self.parse_nodetype_or_name(saved, &mut out.test)?;
                return self.parse_predicates(preds);
            }
        } else {
            out.axis = AXIS_CHILD;
        }

        self.parse_node_test(&mut out.test)?;
        self.parse_predicates(preds)
    }

    /* ---- location paths ---- */

    fn parse_relative_path(&mut self, steps: &mut StepArray) -> PResult {
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

    fn parse_location_path(&mut self) -> PResult<Ast> {
        let mut n = self.new_node(NK_PATH)?;
        let mut steps = StepArray::new();
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
        // SAFETY: a fresh PATH node with no steps yet.
        unsafe {
            let NodeMut::Path(p) = n.payload_mut() else {
                unreachable!("a fresh PATH node")
            };
            p.absolute = c_int::from(absolute);
            steps.install_into_path(n.as_raw());
        }
        Ok(n)
    }

    /* ---- primaries, function calls, filters ---- */

    fn parse_function_call(&mut self, name_tok: Token) -> PResult<Ast> {
        if self.kind() != Tok::LParen {
            return Err(err_setf!(
                self.err,
                XP_ERR_SYNTAX,
                "expected '(' in function call"
            ));
        }
        self.advance()?;
        let mut n = self.new_node(NK_FNCALL)?;
        {
            // SAFETY: a fresh FNCALL node; only its name slots are written.
            let NodeMut::FnCall(f) = (unsafe { n.payload_mut() }) else {
                unreachable!("a fresh FNCALL node")
            };
            if name_tok.kind == Tok::QName {
                /* Each copy lands in the node as it is made, so a failure on the
                 * second leaves the first for the node's guard to free. */
                let (p, l) = split_qname(self.text(&name_tok));
                f.prefix = self.fill_owned(p)?;
                f.name = self.fill_owned(l)?;
            } else {
                let s = self.text(&name_tok);
                f.name = self.fill_owned(s)?;
            }
        }

        let mut args = NodeArray::new();
        if self.kind() != Tok::RParen {
            loop {
                unsafe { limit_check_func_args(self.limits, args.len() + 1, self.err)? };
                let arg = self.parse_expr()?;
                if args.try_push(arg).is_err() {
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
        // SAFETY: the FNCALL node has no arguments yet.
        unsafe { args.install_as_args(n.as_raw()) };
        Ok(n)
    }

    fn parse_primary(&mut self) -> PResult<Ast> {
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
                let mut n = self.new_node(NK_VARREF)?;
                let t = self.tok();
                {
                    // SAFETY: a fresh VARREF node; only its name slots are written.
                    let NodeMut::VarRef(v) = (unsafe { n.payload_mut() }) else {
                        unreachable!("a fresh VARREF node")
                    };
                    if t.kind == Tok::QName {
                        let (p, l) = split_qname(self.text(&t));
                        v.prefix = self.fill_owned(p)?;
                        v.name = self.fill_owned(l)?;
                    } else {
                        let s = self.text(&t);
                        v.name = self.fill_owned(s)?;
                    }
                }
                self.advance()?;
                Ok(n)
            }
            Tok::LParen => {
                self.advance()?;
                let n = self.parse_expr()?;
                self.eat(Tok::RParen, "')' after parenthesised expr")?;
                Ok(n)
            }
            Tok::Literal => {
                let mut n = self.new_node(NK_LITERAL_STR)?;
                let t = self.tok();
                let s = self.text(&t);
                let text = self.fill_owned(s)?;
                // SAFETY: a fresh LITERAL node; only its text slot is written.
                let NodeMut::LiteralStr(slot) = (unsafe { n.payload_mut() }) else {
                    unreachable!("a fresh LITERAL node")
                };
                *slot = text;
                self.advance()?;
                Ok(n)
            }
            Tok::Number => {
                let mut n = self.new_node(NK_LITERAL_NUM)?;
                // SAFETY: a fresh number LITERAL node.
                let num = self.tok().num;
                let NodeMut::LiteralNum(slot) = (unsafe { n.payload_mut() }) else {
                    unreachable!("a fresh number LITERAL node")
                };
                *slot = num;
                self.advance()?;
                Ok(n)
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

    fn parse_filter_expr(&mut self) -> PResult<Ast> {
        let primary = self.parse_primary()?;
        if !matches!(self.kind(), Tok::LBracket | Tok::Slash | Tok::DSlash) {
            return Ok(primary);
        }
        let mut f = self.new_node(NK_FILTER)?;
        // SAFETY: a fresh FILTER node takes sole ownership of `primary`.
        let NodeMut::Filter(filter) = (unsafe { f.payload_mut() }) else {
            unreachable!("a fresh FILTER node")
        };
        filter.expr = primary.into_raw();
        let mut preds = NodeArray::new();
        self.parse_predicates(&mut preds)?;
        /* Optional trailing location path (`$x/foo`, `(expr)//bar`). The shared
         * loop is a no-op when no separator follows. */
        let mut steps = StepArray::new();
        self.parse_step_tail(&mut steps)?;
        // SAFETY: the FILTER node has neither predicates nor a path yet.
        unsafe {
            preds.install_as_filter_preds(f.as_raw());
            steps.install_as_filter_path(f.as_raw());
        }
        Ok(f)
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

    fn parse_path_expr(&mut self) -> PResult<Ast> {
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
    fn make_binop(&mut self, op: u32, lhs: Ast, rhs: Ast) -> PResult<Ast> {
        let mut n = self.new_node(NK_BINOP)?;
        // SAFETY: a fresh BINOP node takes sole ownership of both operands.
        unsafe {
            let NodeMut::BinOp(b) = n.payload_mut() else {
                unreachable!("a fresh BINOP node")
            };
            b.op = op;
            b.lhs = lhs.into_raw();
            b.rhs = rhs.into_raw();
        }
        Ok(n)
    }

    fn parse_union(&mut self) -> PResult<Ast> {
        let mut l = self.parse_path_expr()?;
        while self.kind() == Tok::Pipe {
            self.advance()?;
            let r = self.parse_path_expr()?;
            l = self.make_binop(OP_UNION, l, r)?;
        }
        Ok(l)
    }

    fn parse_unary(&mut self) -> PResult<Ast> {
        let mut neg = false;
        while self.kind() == Tok::Minus {
            neg = !neg;
            self.advance()?;
        }
        let e = self.parse_union()?;
        if !neg {
            return Ok(e);
        }
        let mut u = self.new_node(NK_UNARY)?;
        // SAFETY: a fresh UNARY node takes sole ownership of `e`.
        let NodeMut::Unary(un) = (unsafe { u.payload_mut() }) else {
            unreachable!("a fresh UNARY node")
        };
        un.expr = e.into_raw();
        Ok(u)
    }

    /// Parse binary level `li`, recursing into the tighter levels; the tightest
    /// level's operand is `parse_unary` (prefix '-'). Left-associative.
    fn parse_binary_level(&mut self, li: usize) -> PResult<Ast> {
        let operand = |p: &mut Self| {
            if li == 0 {
                p.parse_unary()
            } else {
                p.parse_binary_level(li - 1)
            }
        };
        let mut l = operand(self)?;
        while let Some(op) = BINOP_LEVELS[li].iter().find_map(|m| {
            let hit = match m.word {
                Some(w) => self.lx.tok_is_word(w),
                None => self.kind() == m.kind,
            };
            hit.then_some(m.op)
        }) {
            self.advance()?;
            let r = operand(self)?;
            l = self.make_binop(op, l, r)?;
        }
        Ok(l)
    }

    fn parse_expr(&mut self) -> PResult<Ast> {
        /* Bound parser recursion so '((((...))))' cannot blow the stack. */
        unsafe { limit_recurse_enter(self.limits, self.err)? };
        let n = self.parse_binary_level(BINOP_LEVELS.len() - 1);
        unsafe { limit_recurse_leave(self.limits) };
        n
    }
}

/* ---- the precedence table ---- */

struct BinMatch {
    /// Some: match a word token ("div", "and", ...); None: match `kind`.
    word: Option<&'static [u8]>,
    kind: Tok,
    op: u32,
}

const fn w(word: &'static [u8], op: u32) -> BinMatch {
    BinMatch {
        word: Some(word),
        kind: Tok::Eof,
        op,
    }
}
const fn k(kind: Tok, op: u32) -> BinMatch {
    BinMatch {
        word: None,
        kind,
        op,
    }
}

/// Tightest-binding level first.
static BINOP_LEVELS: &[&[BinMatch]] = &[
    &[k(Tok::Star, OP_MUL), w(b"div", OP_DIV), w(b"mod", OP_MOD)],
    &[k(Tok::Plus, OP_ADD), k(Tok::Minus, OP_SUB)],
    &[
        k(Tok::Lt, OP_LT),
        k(Tok::Gt, OP_GT),
        k(Tok::Le, OP_LE),
        k(Tok::Ge, OP_GE),
    ],
    &[k(Tok::Eq, OP_EQ), k(Tok::Ne, OP_NE)],
    &[w(b"and", OP_AND)],
    &[w(b"or", OP_OR)],
];

/* ---- entry ---- */

/// Parse an expression into a Rust-owned compiled AST; `Err` with `*err`
/// filled on failure.
///
/// `expr` is a verified text: NUL-free, valid UTF-8.
///
/// # Safety
/// `limits` must be null or live, and `err`'s slot live.
pub(crate) unsafe fn parse_owned(
    expr: VerifiedText,
    limits: *mut Limits,
    err: ErrSink,
) -> Result<Ast, Reported> {
    if limits.is_null() {
        return Err(err_setf!(
            err,
            XP_ERR_INTERNAL,
            "parse_raw: limits required"
        ));
    }
    limit_check_expr_bytes(limits, expr.len(), err)?;

    let src: &[u8] = unsafe { expr.as_bytes() };

    let lx = Lexer::new(src).map_err(|e| lex_err(err, e))?;
    let mut p = Parser { lx, err, limits };

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
    /* Peephole first, so the hoisting pass sees the rewritten step structure. */
    apply_peephole(root.as_raw());
    mark_context_independent(root.as_raw());
    Ok(root)
}

/// Parse an expression into a compiled AST; NULL on error with `*err` filled.
///
/// # Safety
/// As [`parse_owned`], with `err` null or a writable error slot; the caller
/// frees the result with `node_free`.
pub unsafe fn parse_raw(expr: VerifiedText, limits: *mut Limits, err: *mut Error) -> *mut Node {
    parse_owned(expr, limits, ErrSink::from_raw(err)).map_or(ptr::null_mut(), Ast::into_raw)
}
