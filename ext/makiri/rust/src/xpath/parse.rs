//! XPath 1.0 recursive-descent parser (mkr_xpath_parse.c).
//!
//! It builds the C AST directly, through the C allocator, so the C evaluator
//! runs what it produces and the CSS lowering keeps sharing one node factory.
//! That is what the `unsafe` here is: writing C-layout nodes and the arrays
//! hanging off them. The ownership discipline is the C one unchanged - a
//! half-built node is always in a state `mkr_node_free` / `mkr_step_clear` can
//! take apart, so every error path bails without cleanup of its own.
//!
//! Lookahead is one token, except where the grammar needs two (a NAME that may
//! be an axis, a node-type keyword, or a function name), which is done by
//! advancing and keeping the token that was there.

use super::abi::*;
use super::msg::Bytes;
use super::lex::{LexErr, Lexer, Tok, Token};
use crate::err_setf;
use core::ffi::{c_char, c_void};
use core::ptr;

struct Parser<'a> {
    lx: Lexer<'a>,
    err: *mut Error,
    limits: *mut Limits,
}

/// Report a lexer failure as an `mkr_xpath_error_t`. A free function because
/// the very first token is lexed before there is a parser to hold it.
fn lex_err(err: *mut Error, e: LexErr) {
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

/// A step under construction. Mirrors the C `mkr_step_t {0}` local: zeroed, and
/// cleared through `mkr_step_clear` if the parse of it fails.
fn zero_step() -> Step {
    Step {
        axis: AXIS_CHILD,
        test: NodeTest {
            kind: NT_NAME,
            prefix: OwnedText { ptr: ptr::null_mut(), len: 0 },
            local: OwnedText { ptr: ptr::null_mut(), len: 0 },
            pi_target: OwnedText { ptr: ptr::null_mut(), len: 0 },
        },
        predicates: ptr::null_mut(),
        npredicates: 0,
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
    matches!(s, b"node" | b"text" | b"comment" | b"processing-instruction")
}

/// A growable array living in an AST node's (pointer, count) slots.
///
/// It writes through to the node on every push rather than holding the array
/// until it is complete, because that is the ownership contract: a parse that
/// fails partway leaves the elements already pushed reachable from the node,
/// and `mkr_node_free` is what frees them. The capacity is the only part the
/// node does not store, so it rides here.
///
/// The three arrays an AST carries - a path's steps, a step's predicates, a
/// call's arguments - differ only in element type and in which budget they
/// charge, so the growing itself lives here once.
struct Slots<T> {
    ptr: *mut *mut T,
    len: *mut usize,
    /// What this value knows it has reserved - a lower bound on the real
    /// capacity, not the capacity itself. A fresh `Slots` over a populated pair
    /// starts at 0 and simply reallocates once more than it had to; the grower
    /// reallocates from the existing pointer, so nothing is lost.
    cap: usize,
}

impl<T> Slots<T> {
    /// Take over a node's slots, which are already empty - `mkr_node_alloc`
    /// zeroes a new node, and `zero_step` a new step.
    ///
    /// # Safety
    /// Both must point into a live AST node that outlives this value.
    unsafe fn at(ptr: *mut *mut T, len: *mut usize) -> Slots<T> {
        Slots { ptr, len, cap: 0 }
    }

    unsafe fn len(&self) -> usize {
        *self.len
    }

    /// Append, growing through the C grower so `mkr_node_free` can free it.
    /// False on OOM, with `*err` naming the array.
    unsafe fn push(&mut self, v: T, err: *mut Error, what: &str) -> bool {
        if mkr_grow_reserve(
            self.ptr as *mut *mut c_void,
            &mut self.cap,
            *self.len + 1,
            core::mem::size_of::<T>(),
        ) != MKR_OK
        {
            err_setf!(err, XP_ERR_OOM, "out of memory growing {} array", what);
            return false;
        }
        *(*self.ptr).add(*self.len) = v;
        *self.len += 1;
        true
    }
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

    fn advance(&mut self) -> bool {
        match self.lx.advance() {
            Ok(()) => true,
            Err(e) => {
                lex_err(self.err, e);
                false
            }
        }
    }

    fn eat(&mut self, k: Tok, what: &str) -> bool {
        if self.kind() != k {
            err_setf!(self.err, XP_ERR_SYNTAX, "expected {}", what);
            return false;
        }
        self.advance()
    }

    fn new_node(&mut self, kind: u32) -> *mut Node {
        unsafe { mkr_node_alloc(self.limits, self.err, kind) }
    }

    /// Copy `text` into an owned-text AST slot. A failure must be propagated: a
    /// null slot left in the AST would silently mis-compare at evaluation, so
    /// the parse fails closed instead.
    fn fill_owned(&mut self, text: &[u8], out: *mut OwnedText) -> bool {
        let p = unsafe { mkr_strndup(text.as_ptr() as *const c_char, text.len()) };
        if p.is_null() {
            err_setf!(self.err, XP_ERR_OOM, "out of memory in parser");
            unsafe { (*out).len = 0 };
            return false;
        }
        unsafe {
            (*out).ptr = p;
            (*out).len = text.len();
        }
        true
    }

    /// Split a QNAME token into prefix and local, and copy both.
    fn fill_qname_split(&mut self, t: &Token, prefix: *mut OwnedText, local: *mut OwnedText) -> bool {
        let (p, l) = split_qname(self.text(t));
        self.fill_owned(p, prefix) && self.fill_owned(l, local)
    }

    fn push_step(&mut self, steps: &mut Slots<Step>, s: Step) -> bool {
        unsafe {
            mkr_limit_check_steps(self.limits, steps.len() + 1, self.err) == 0
                && steps.push(s, self.err, "step")
        }
    }

    /// Parse a run of `('/' | '//') Step`, expanding each `//` into an implicit
    /// `descendant-or-self::node()` step. A non-separator token makes this a
    /// no-op. Steps pushed before a failure stay in the array - the owning node
    /// frees them.
    fn parse_step_tail(&mut self, steps: &mut Slots<Step>) -> bool {
        while self.kind() == Tok::Slash || self.kind() == Tok::DSlash {
            let dslash = self.kind() == Tok::DSlash;
            if !self.advance() {
                return false;
            }
            if dslash {
                let mut implicit = zero_step();
                implicit.axis = AXIS_DESCENDANT_OR_SELF;
                implicit.test.kind = NT_NODE;
                if !self.push_step(steps, implicit) {
                    return false;
                }
            }
            let mut next = zero_step();
            if !self.parse_step(&mut next) {
                return false;
            }
            if !self.push_step(steps, next) {
                unsafe { mkr_step_clear(&mut next) };
                return false;
            }
        }
        true
    }

    /* ---- node tests ---- */

    /// `saved` is an already-consumed NAME; the current token is the one after
    /// it. A node-type keyword immediately followed by '(' is a node-type test
    /// (with an optional PI target literal); anything else is an NCName test.
    fn parse_nodetype_or_name(&mut self, saved: Token, out: *mut NodeTest) -> bool {
        let name = self.text(&saved);
        if is_nodetype_name(name) && self.kind() == Tok::LParen {
            if !self.advance() {
                return false;
            }
            match name {
                b"node" => unsafe { (*out).kind = NT_NODE },
                b"text" => unsafe { (*out).kind = NT_TEXT },
                b"comment" => unsafe { (*out).kind = NT_COMMENT },
                _ => {
                    unsafe { (*out).kind = NT_PI };
                    if self.kind() == Tok::Literal {
                        let t = self.tok();
                        let s = self.text(&t);
                        if !self.fill_owned(s, unsafe { &raw mut (*out).pi_target }) {
                            return false;
                        }
                        if !self.advance() {
                            return false;
                        }
                    }
                }
            }
            return self.eat(Tok::RParen, "')' after node type test");
        }
        unsafe { (*out).kind = NT_NAME };
        self.fill_owned(name, unsafe { &raw mut (*out).local })
    }

    /// Called with the current token at the first token of the node test; leaves
    /// it at the token after.
    fn parse_node_test(&mut self, out: *mut NodeTest) -> bool {
        unsafe { *out = zero_step().test };

        if self.kind() == Tok::Star {
            unsafe { (*out).kind = NT_WILDCARD };
            return self.advance();
        }
        if self.kind() == Tok::Name {
            let saved = self.tok();
            if !self.advance() {
                return false;
            }
            return self.parse_nodetype_or_name(saved, out);
        }
        if self.kind() == Tok::QName {
            /* `prefix:local` or `prefix:*`. */
            let t = self.tok();
            let (prefix, local) = split_qname(self.text(&t));
            if !self.fill_owned(prefix, unsafe { &raw mut (*out).prefix }) {
                return false;
            }
            if local == b"*" {
                unsafe { (*out).kind = NT_WILDCARD };
            } else {
                unsafe { (*out).kind = NT_NAME };
                if !self.fill_owned(local, unsafe { &raw mut (*out).local }) {
                    return false;
                }
            }
            return self.advance();
        }
        err_setf!(self.err, XP_ERR_SYNTAX, "expected node test");
        false
    }

    /* ---- predicates ---- */

    fn parse_predicates(&mut self, preds: &mut Slots<*mut Node>) -> bool {
        while self.kind() == Tok::LBracket {
            unsafe {
                if mkr_limit_check_predicates(self.limits, preds.len() + 1, self.err) != 0 {
                    return false;
                }
            }
            if !self.advance() {
                return false;
            }
            let e = self.parse_expr();
            if e.is_null() {
                return false;
            }
            if !self.eat(Tok::RBracket, "']' to close predicate") {
                unsafe { mkr_node_free(e) };
                return false;
            }
            unsafe {
                if !preds.push(e, self.err, "predicate") {
                    mkr_node_free(e);
                    return false;
                }
            }
        }
        true
    }

    /* ---- steps ---- */

    /// On failure the step is cleared here: `parse_step_inner` can leave a
    /// partly-built step behind (an owned name already copied, predicates
    /// already pushed) and every caller just bails, so freeing it in one place
    /// keeps the error paths leak-free without per-site cleanup.
    fn parse_step(&mut self, out: *mut Step) -> bool {
        if self.parse_step_inner(out) {
            return true;
        }
        unsafe { mkr_step_clear(out) };
        false
    }

    fn parse_step_inner(&mut self, out: *mut Step) -> bool {
        unsafe { *out = zero_step() };

        /* Abbreviated steps. */
        if self.kind() == Tok::Dot {
            if !self.advance() {
                return false;
            }
            unsafe {
                (*out).axis = AXIS_SELF;
                (*out).test.kind = NT_NODE;
            }
            return true;
        }
        if self.kind() == Tok::DotDot {
            if !self.advance() {
                return false;
            }
            unsafe {
                (*out).axis = AXIS_PARENT;
                (*out).test.kind = NT_NODE;
            }
            return true;
        }

        /* AxisSpecifier: '@' or NAME '::'. */
        if self.kind() == Tok::At {
            unsafe { (*out).axis = AXIS_ATTRIBUTE };
            if !self.advance() {
                return false;
            }
        } else if self.kind() == Tok::Name {
            /* Axis or NameTest - decided by the token after, so peek by
             * advancing and keeping the NAME. */
            let saved = self.tok();
            if !self.advance() {
                return false;
            }
            if self.kind() == Tok::ColonColon {
                let name = self.text(&saved);
                match axis_by_name(name) {
                    Some(ax) => unsafe { (*out).axis = ax },
                    None => {
                        err_setf!(
                            self.err,
                            XP_ERR_SYNTAX,
                            "unknown axis '{}'",
                            Bytes(name)
                        );
                        return false;
                    }
                }
                if !self.advance() {
                    return false; /* eat '::' */
                }
            } else {
                /* It was a NameTest, and the NAME is already consumed, so replay
                 * it through the shared node-type grammar. */
                unsafe { (*out).axis = AXIS_CHILD };
                if !self.parse_nodetype_or_name(saved, unsafe { &raw mut (*out).test }) {
                    return false;
                }
                return self.parse_predicates(&mut unsafe {
                    Slots::at(&raw mut (*out).predicates, &raw mut (*out).npredicates)
                });
            }
        } else {
            unsafe { (*out).axis = AXIS_CHILD };
        }

        if !self.parse_node_test(unsafe { &raw mut (*out).test }) {
            return false;
        }
        self.parse_predicates(&mut unsafe {
            Slots::at(&raw mut (*out).predicates, &raw mut (*out).npredicates)
        })
    }

    /* ---- location paths ---- */

    fn parse_relative_path(&mut self, steps: &mut Slots<Step>) -> bool {
        let mut s = zero_step();
        if !self.parse_step(&mut s) {
            return false;
        }
        if !self.push_step(steps, s) {
            unsafe { mkr_step_clear(&mut s) };
            return false;
        }
        self.parse_step_tail(steps)
    }

    fn can_start_step(&self) -> bool {
        matches!(
            self.kind(),
            Tok::Dot | Tok::DotDot | Tok::At | Tok::Star | Tok::Name | Tok::QName
        )
    }

    fn parse_location_path(&mut self) -> *mut Node {
        let n = self.new_node(NK_PATH);
        if n.is_null() {
            return n;
        }
        let path = unsafe { &raw mut (*n).u.path };
        let mut steps = unsafe { Slots::at(&raw mut (*path).steps, &raw mut (*path).nsteps) };

        if self.kind() == Tok::Slash {
            unsafe { (*path).absolute = 1 };
            if !self.advance() {
                unsafe { mkr_node_free(n) };
                return ptr::null_mut();
            }
            if self.can_start_step() && !self.parse_relative_path(&mut steps) {
                unsafe { mkr_node_free(n) };
                return ptr::null_mut();
            }
            return n;
        }
        if self.kind() == Tok::DSlash {
            /* '//' = '/descendant-or-self::node()/'. Leave the token where it is
             * so the shared loop expands it itself. */
            unsafe { (*path).absolute = 1 };
            if !self.parse_step_tail(&mut steps) {
                unsafe { mkr_node_free(n) };
                return ptr::null_mut();
            }
            return n;
        }
        unsafe { (*path).absolute = 0 };
        if !self.parse_relative_path(&mut steps) {
            unsafe { mkr_node_free(n) };
            return ptr::null_mut();
        }
        n
    }

    /* ---- primaries, function calls, filters ---- */

    fn parse_function_call(&mut self, name_tok: Token) -> *mut Node {
        if self.kind() != Tok::LParen {
            err_setf!(self.err, XP_ERR_SYNTAX, "expected '(' in function call");
            return ptr::null_mut();
        }
        if !self.advance() {
            return ptr::null_mut();
        }
        let n = self.new_node(NK_FNCALL);
        if n.is_null() {
            return n;
        }
        let f = unsafe { &raw mut (*n).u.fncall };

        let named = if name_tok.kind == Tok::QName {
            self.fill_qname_split(&name_tok, unsafe { &raw mut (*f).prefix }, unsafe {
                &raw mut (*f).name
            })
        } else {
            let s = self.text(&name_tok);
            self.fill_owned(s, unsafe { &raw mut (*f).name })
        };
        if !named {
            unsafe { mkr_node_free(n) };
            return ptr::null_mut();
        }

        let mut args = unsafe { Slots::at(&raw mut (*f).args, &raw mut (*f).nargs) };
        if self.kind() != Tok::RParen {
            loop {
                unsafe {
                    if mkr_limit_check_func_args(self.limits, args.len() + 1, self.err) != 0 {
                        mkr_node_free(n);
                        return ptr::null_mut();
                    }
                }
                let arg = self.parse_expr();
                if arg.is_null() {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
                unsafe {
                    if !args.push(arg, self.err, "function argument") {
                        mkr_node_free(arg);
                        mkr_node_free(n);
                        return ptr::null_mut();
                    }
                }
                if self.kind() != Tok::Comma {
                    break;
                }
                if !self.advance() {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
            }
        }
        if !self.eat(Tok::RParen, "')' after function arguments") {
            unsafe { mkr_node_free(n) };
            return ptr::null_mut();
        }
        n
    }

    fn parse_primary(&mut self) -> *mut Node {
        match self.kind() {
            Tok::Dollar => {
                if !self.advance() {
                    return ptr::null_mut();
                }
                if self.kind() != Tok::Name && self.kind() != Tok::QName {
                    err_setf!(self.err, XP_ERR_SYNTAX, "expected name after '$'");
                    return ptr::null_mut();
                }
                let n = self.new_node(NK_VARREF);
                if n.is_null() {
                    return n;
                }
                let v = unsafe { &raw mut (*n).u.varref };
                let t = self.tok();
                let named = if t.kind == Tok::QName {
                    self.fill_qname_split(&t, unsafe { &raw mut (*v).prefix }, unsafe {
                        &raw mut (*v).name
                    })
                } else {
                    let s = self.text(&t);
                    self.fill_owned(s, unsafe { &raw mut (*v).name })
                };
                if !named || !self.advance() {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
                n
            }
            Tok::LParen => {
                if !self.advance() {
                    return ptr::null_mut();
                }
                let n = self.parse_expr();
                if n.is_null() {
                    return n;
                }
                if !self.eat(Tok::RParen, "')' after parenthesised expr") {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
                n
            }
            Tok::Literal => {
                let n = self.new_node(NK_LITERAL_STR);
                if n.is_null() {
                    return n;
                }
                let t = self.tok();
                let s = self.text(&t);
                if !self.fill_owned(s, unsafe { &raw mut (*n).u.literal }) || !self.advance() {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
                n
            }
            Tok::Number => {
                let n = self.new_node(NK_LITERAL_NUM);
                if n.is_null() {
                    return n;
                }
                unsafe { (*n).u.literal_num = self.tok().num };
                if !self.advance() {
                    unsafe { mkr_node_free(n) };
                    return ptr::null_mut();
                }
                n
            }
            Tok::Name | Tok::QName => {
                let name_tok = self.tok();
                if !self.advance() {
                    return ptr::null_mut();
                }
                if self.kind() == Tok::LParen {
                    return self.parse_function_call(name_tok);
                }
                err_setf!(self.err, XP_ERR_SYNTAX, "expected '(' after function name");
                ptr::null_mut()
            }
            _ => {
                err_setf!(self.err, XP_ERR_SYNTAX, "expected primary expression");
                ptr::null_mut()
            }
        }
    }

    fn parse_filter_expr(&mut self) -> *mut Node {
        let primary = self.parse_primary();
        if primary.is_null() {
            return primary;
        }
        if !matches!(self.kind(), Tok::LBracket | Tok::Slash | Tok::DSlash) {
            return primary;
        }
        let f = self.new_node(NK_FILTER);
        if f.is_null() {
            unsafe { mkr_node_free(primary) };
            return ptr::null_mut();
        }
        let fl = unsafe { &raw mut (*f).u.filter };
        unsafe { (*fl).expr = primary };
        if !self.parse_predicates(&mut unsafe { Slots::at(&raw mut (*fl).preds, &raw mut (*fl).npreds) })
        {
            unsafe { mkr_node_free(f) };
            return ptr::null_mut();
        }
        /* Optional trailing location path (`$x/foo`, `(expr)//bar`). The shared
         * loop is a no-op when no separator follows. */
        if !self.parse_step_tail(&mut unsafe {
            Slots::at(&raw mut (*fl).path_steps, &raw mut (*fl).npath)
        }) {
            unsafe { mkr_node_free(f) };
            return ptr::null_mut();
        }
        f
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

    fn parse_path_expr(&mut self) -> *mut Node {
        if self.kind() == Tok::Slash || self.kind() == Tok::DSlash {
            return self.parse_location_path();
        }
        if self.looks_like_filter_expr() {
            return self.parse_filter_expr();
        }
        self.parse_location_path()
    }

    /* ---- the operator ladder ---- */

    fn make_binop(&mut self, op: u32, lhs: *mut Node, rhs: *mut Node) -> *mut Node {
        let n = self.new_node(NK_BINOP);
        if n.is_null() {
            /* The caller owns lhs/rhs and frees them on a null return. */
            return n;
        }
        unsafe {
            (*n).u.binop.op = op;
            (*n).u.binop.lhs = lhs;
            (*n).u.binop.rhs = rhs;
        }
        n
    }

    fn parse_union(&mut self) -> *mut Node {
        let mut l = self.parse_path_expr();
        while !l.is_null() && self.kind() == Tok::Pipe {
            if !self.advance() {
                unsafe { mkr_node_free(l) };
                return ptr::null_mut();
            }
            let r = self.parse_path_expr();
            if r.is_null() {
                unsafe { mkr_node_free(l) };
                return ptr::null_mut();
            }
            let u = self.make_binop(OP_UNION, l, r);
            if u.is_null() {
                unsafe {
                    mkr_node_free(l);
                    mkr_node_free(r);
                }
                return ptr::null_mut();
            }
            l = u;
        }
        l
    }

    fn parse_unary(&mut self) -> *mut Node {
        let mut neg = false;
        while self.kind() == Tok::Minus {
            neg = !neg;
            if !self.advance() {
                return ptr::null_mut();
            }
        }
        let e = self.parse_union();
        if e.is_null() || !neg {
            return e;
        }
        let u = self.new_node(NK_UNARY);
        if u.is_null() {
            unsafe { mkr_node_free(e) };
            return ptr::null_mut();
        }
        unsafe { (*u).u.unary.expr = e };
        u
    }

    /// Parse binary level `li`, recursing into the tighter levels; the tightest
    /// level's operand is `parse_unary` (prefix '-'). Left-associative.
    fn parse_binary_level(&mut self, li: usize) -> *mut Node {
        let operand = |p: &mut Self| {
            if li == 0 {
                p.parse_unary()
            } else {
                p.parse_binary_level(li - 1)
            }
        };
        let mut l = operand(self);
        while !l.is_null() {
            let Some(op) = BINOP_LEVELS[li].iter().find_map(|m| {
                let hit = match m.word {
                    Some(w) => self.lx.tok_is_word(w),
                    None => self.kind() == m.kind,
                };
                hit.then_some(m.op)
            }) else {
                break;
            };
            if !self.advance() {
                unsafe { mkr_node_free(l) };
                return ptr::null_mut();
            }
            let r = operand(self);
            if r.is_null() {
                unsafe { mkr_node_free(l) };
                return ptr::null_mut();
            }
            let b = self.make_binop(op, l, r);
            if b.is_null() {
                unsafe {
                    mkr_node_free(l);
                    mkr_node_free(r);
                }
                return ptr::null_mut();
            }
            l = b;
        }
        l
    }

    fn parse_expr(&mut self) -> *mut Node {
        /* Bound parser recursion so '((((...))))' cannot blow the stack. */
        unsafe {
            if mkr_limit_recurse_enter(self.limits, self.err) != 0 {
                return ptr::null_mut();
            }
        }
        let n = self.parse_binary_level(BINOP_LEVELS.len() - 1);
        unsafe { mkr_limit_recurse_leave(self.limits) };
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
    BinMatch { word: Some(word), kind: Tok::Eof, op }
}
const fn k(kind: Tok, op: u32) -> BinMatch {
    BinMatch { word: None, kind, op }
}

/// Tightest-binding level first.
static BINOP_LEVELS: &[&[BinMatch]] = &[
    &[k(Tok::Star, OP_MUL), w(b"div", OP_DIV), w(b"mod", OP_MOD)],
    &[k(Tok::Plus, OP_ADD), k(Tok::Minus, OP_SUB)],
    &[k(Tok::Lt, OP_LT), k(Tok::Gt, OP_GT), k(Tok::Le, OP_LE), k(Tok::Ge, OP_GE)],
    &[k(Tok::Eq, OP_EQ), k(Tok::Ne, OP_NE)],
    &[w(b"and", OP_AND)],
    &[w(b"or", OP_OR)],
];

/* ---- entry ---- */

/// Parse an expression into a compiled AST; NULL on error with `*err` filled.
///
/// `expr` is a verified text: NUL-free, NUL-terminated, valid UTF-8.
/// # Safety
/// A C entry point: the contract is the one at its declaration in
/// ext/makiri/xpath/mkr_xpath*.h.
pub unsafe extern "C" fn mkr_parse(
    expr: VerifiedText,
    limits: *mut Limits,
    err: *mut Error,
) -> *mut Node {
    if limits.is_null() {
        err_setf!(err, XP_ERR_INTERNAL, "mkr_parse: limits required");
        return ptr::null_mut();
    }
    if mkr_limit_check_expr_bytes(limits, expr.len, err) != 0 {
        return ptr::null_mut();
    }

    let src: &[u8] = if expr.ptr.is_null() || expr.len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(expr.ptr as *const u8, expr.len)
    };

    let lx = match Lexer::new(src) {
        Ok(lx) => lx,
        Err(e) => {
            lex_err(err, e);
            return ptr::null_mut();
        }
    };
    let mut p = Parser { lx, err, limits };

    let root = p.parse_expr();
    if root.is_null() {
        return ptr::null_mut();
    }
    if p.kind() != Tok::Eof {
        let t = p.tok();
        err_setf!(err, XP_ERR_SYNTAX, "trailing input at '{}'", Bytes(p.text(&t)));
        mkr_node_free(root);
        return ptr::null_mut();
    }
    /* Peephole first, so the hoisting pass sees the rewritten step structure. */
    mkr_apply_peephole(root);
    mkr_mark_context_independent(root);
    root
}
