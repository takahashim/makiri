//! The selector parser: Lexbor's rule machinery (`css/parser.c`,
//! `css/syntax/parser.c`, `css/syntax/syntax.c`, `css/state.c`) and the
//! selector states over it (`css/selectors/state.c`, `pseudo_state.c`,
//! `selectors.c`, the list and selector links of `selector.c`, and
//! `css/syntax/anb.c`).
//!
//! The port keeps Lexbor's control flow: a rule (a component list or a
//! function's argument) has a phase that filters every token the states read,
//! counting brackets and turning the token that ends the rule into `_END`,
//! and a state that consumes tokens. A state returns `false` to be called
//! again on the same token. The state stack holds where to return when a
//! nested part ends. What changed, and only for ownership:
//!
//! - **Function pointers are enums** (`St`, `Phase`, `Back`, `EndCb`), with
//!   one variant per Lexbor function (several Lexbor functions that are
//!   identical share one), so that a state compares as Lexbor's pointer does.
//! - **Pointers are indices**: the stacks are `Vec`s with a top index, and
//!   the selector graph is the [`Ast`] arena. A context (`void *ctx`) is a
//!   list id.
//! - **Lexbor's log is not kept**: nothing reads it.
//!
//! There is no native recursion: a nested selector list is a rule and a state
//! pushed on the heap stacks, as in Lexbor. The stylesheet rules (rule
//! lists, at-rules, qualified rules, blocks, declarations) are phases and
//! states on the same stacks, in the `syntax` submodule.

use super::ast::{
    comb, matching, modifier, sel_type, AnbOf, Ast, ListId, ListNode, PseudoData, SelId, SelNode,
};
use super::pseudo::{self, class, class_function, element, FuncData};
use super::rules::{AtKind, ItemId, Rules};
use super::token::{Span, Token, TokenType as T};
use super::tokenizer::Tokenizer;
use super::Status;
use crate::falloc::{Reserve, VecPush};
use core::ffi::{c_long, c_ulong};

mod syntax;
pub use syntax::{parse_stylesheet, Sheet};

/// A state's context: the selector list Lexbor passes as `void *ctx`.
pub type Ctx = Option<ListId>;

/// `lxb_css_parser_state_f`: the states this parser runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    /// A state Lexbor leaves NULL and never calls.
    Stub,
    /// `lxb_css_state_success`.
    Success,
    /// `lxb_css_state_failed`.
    Failed,
    /// `lxb_css_state_stop`.
    Stop,
    ComplexList,
    ComplexListEnd,
    RelativeList,
    RelativeListEnd,
    RelativeListWoRoot,
    ComplexWoRoot,
    ComplexEnd,
    CompoundWoRoot,
    CompoundHandler,
    CompoundSub,
    CompoundPseudo,
    /// `lxb_css_selectors_state_pseudo_class_function__undef`.
    PseudoClassFunctionUndef,
    /// `..._dir`, `..._lang`: refused before their argument is read.
    PseudoClassFunctionUnsupported,
    /// `..._current`, `..._is`, `..._not`, `..._where`.
    PseudoClassFunctionList,
    /// `..._has`.
    PseudoClassFunctionHas,
    /// `lxb_css_selectors_state_pseudo_anb_begin` (the `:nth-*` without
    /// `of S`).
    PseudoAnbBegin,
    /// `lxb_css_selectors_state_pseudo_of_begin`.
    PseudoOfBegin,
    /// `lxb_css_selectors_state_pseudo_of_back`.
    PseudoOfBack,
    /// `lxb_css_selectors_state_pseudo_element_function__undef`.
    PseudoElementFunctionUndef,
    /// `lxb_css_state_blank`.
    Blank,
    /// `lxb_css_state_list_rules_next`.
    ListRulesNext,
    /// The at-rule preludes of `css/at_rule/state.c`: every kind skips to
    /// the end, `font-face` fails on anything but an empty prelude.
    AtPrelude,
    /// The prelude-failed states (`lxb_css_at_rule_*_prelude_failed`,
    /// `lxb_css_state_qualified_rule_prelude_failed`), which are one loop.
    SkipToEnd,
    /// `lxb_css_state_qualified_rule_prelude`.
    QualifiedPrelude,
    /// `lxb_css_state_block_next`.
    BlockNext,
    /// A declaration's value. Lexbor runs the property's own state here
    /// (`entry->state`); the port reads every value as Lexbor reads a custom
    /// property's (`lxb_css_property_state__custom`), since the reader takes
    /// the value from its offsets (docs/rewrite_parser.ja.md).
    DeclarationValue,
    /// `lxb_css_state_declarations_bad`.
    DeclarationsBad,
}

/// `lxb_css_syntax_cb_done_f`: the callbacks a rule runs as it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndCb {
    /// `lxb_css_selectors_components_end`.
    ComponentsEnd,
    /// `lxb_css_selectors_state_function_end`.
    FunctionEnd,
    /// `lxb_css_selectors_state_function_forgiving`.
    FunctionForgiving,
    /// `lxb_css_selectors_state_function_forgiving_relative`.
    FunctionForgivingRelative,
    /// `lxb_css_selectors_state_pseudo_of_end`.
    PseudoOfEnd,
    /// `lxb_css_state_list_rules_end`.
    ListRulesEnd,
    /// The at-rule ends of `css/at_rule/state.c`, by the kind the rule was
    /// created as.
    AtRuleEnd(AtKind),
    /// `lxb_css_state_qualified_rule_end`.
    QualifiedEnd,
    /// `lxb_css_state_block_end`.
    BlockEnd,
    /// `lxb_css_state_declarations_end`.
    DeclarationsEnd,
}

/// `rule->phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Stub,
    Components,
    Function,
    End,
    EndConsumeToken,
    ListRules,
    ListRulesAt,
    ListRulesQualified,
    AtRule,
    AtBlock,
    AtPreludeEnd,
    AtPreludeEndConsumeToken,
    Qualified,
    QualifiedBlock,
    QualifiedPreludeEnd,
    Block,
    BlockAt,
    BlockDeclarations,
    BlockQualified,
    DeclarationsBegin,
    DeclarationsName,
    DeclarationsValue,
    DeclarationsNext,
    DeclarationEnd,
    DeclarationsEnd,
}

/// `rule->back`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Back {
    Stub,
    /// `lxb_css_syntax_parser_components_back_to_value`.
    Components,
    /// `lxb_css_syntax_parser_function_back`.
    Function,
    /// `lxb_css_syntax_parser_list_rules_back`.
    ListRules,
    /// `lxb_css_syntax_parser_at_back_to_prelude`.
    AtToPrelude,
    /// `lxb_css_syntax_parser_qualified_back_to_prelude`.
    QualifiedToPrelude,
    /// `lxb_css_syntax_parser_block_back`.
    Block,
    /// `lxb_css_syntax_parser_declaration_back_to_value`.
    DeclarationToValue,
    /// `lxb_css_syntax_parser_end_back`.
    ToEnd,
}

/// `lxb_css_syntax_rule_t`.
///
/// Lexbor's one `context` pointer is two fields here: `context` is the
/// selector list the selector states pass along, `item` the rule a
/// stylesheet callback builds (`context_old` is `item_old`). No state reads
/// the other's, so the split changes nothing Lexbor does.
#[derive(Clone, Copy, Debug)]
struct Rule {
    phase: Phase,
    state: St,
    back: Back,
    back_state: St,
    context: Ctx,
    /// `cbx.cb->failed`.
    failed_state: St,
    /// `cbx.cb->end`.
    end: EndCb,
    offset: usize,
    deep: usize,
    block_end: T,
    skip_consume: bool,
    failed: bool,
    item: Option<ItemId>,
    item_old: Option<ItemId>,
    returned: Option<ItemId>,
    begin: usize,
    nested: bool,
    important: bool,
}

impl Default for Rule {
    fn default() -> Rule {
        Rule {
            phase: Phase::Stub,
            state: St::Stub,
            back: Back::Stub,
            back_state: St::Stub,
            context: None,
            failed_state: St::Stub,
            end: EndCb::ComponentsEnd,
            offset: 0,
            deep: 0,
            block_end: T::Undef,
            skip_consume: false,
            failed: false,
            item: None,
            item_old: None,
            returned: None,
            begin: 0,
            nested: false,
            important: false,
        }
    }
}

/// `lxb_css_syntax_declaration_offset_t`, the parser's `offset`.
#[derive(Clone, Copy, Debug, Default)]
struct DeclOffset {
    value_begin: usize,
    value_end: usize,
    important_begin: usize,
    important_end: usize,
    end: usize,
}

/// `lxb_css_parser_state_t`.
#[derive(Clone, Copy, Debug)]
struct StateEntry {
    state: St,
    context: Ctx,
    root: bool,
}

/// `lxb_css_syntax_cb_components_t`.
#[derive(Clone, Copy)]
struct Components {
    prelude: St,
    failed: St,
    end: EndCb,
}

/// `lxb_css_selectors_t`.
#[derive(Clone, Copy, Debug)]
struct Selectors {
    list: Option<ListId>,
    list_last: Option<ListId>,
    parent: Option<SelId>,
    combinator: u8,
    comb_default: u8,
    err_in_function: bool,
}

/// `lxb_css_syntax_anb_t`.
#[derive(Clone, Copy, Default)]
struct Anb {
    a: c_long,
    b: c_long,
}

/* ---- specificity: `lxb_css_selector_sp_*` ---- */

const SP_B_MAX: u32 = (1 << 18) - 1;
const SP_C_MAX: u32 = (1 << 9) - 1;

fn sp_get(sp: u32, n: u32) -> u32 {
    (sp >> n) & 0x1FF
}

fn sp_set(sp: u32, n: u32, num: u32) -> u32 {
    (sp & !(0x1FF << n)) | num.wrapping_shl(n)
}

/// `lexbor_conv_double_to_long`. Lexbor casts a value of exactly 2^63 with
/// C's undefined conversion; this saturates it (docs/parser_differences.ja.md).
fn double_to_long(number: f64) -> c_long {
    if number > c_long::MAX as f64 {
        return c_long::MAX;
    }
    if number < c_long::MIN as f64 {
        return -c_long::MAX;
    }
    number as c_long
}

/// `lexbor_conv_data_to_long`: returns the value and how many bytes it read.
fn data_to_long(data: &[u8]) -> (c_long, usize) {
    let mut minus = false;
    let mut p = 0;
    match data.first() {
        Some(b'-') => {
            minus = true;
            p += 1;
        }
        Some(b'+') => p += 1,
        _ => {}
    }
    let mut number: c_ulong = 0;
    while p < data.len() {
        let d = data[p];
        if !d.is_ascii_digit() {
            break;
        }
        let n = c_ulong::from(d - b'0').wrapping_add(number.wrapping_mul(10));
        if n > c_long::MAX as c_ulong {
            p -= 1;
            break;
        }
        number = n;
        p += 1;
    }
    let value = if minus { number.wrapping_neg() } else { number };
    (value as c_long, p)
}

/// A parsed selector list and the arena it lives in.
pub struct Parsed {
    pub ast: Ast,
    pub first: Option<ListId>,
}

impl Parsed {
    /// The selector's comma groups, in order.
    pub fn groups(&self) -> super::ast::Lists<'_> {
        self.ast.lists_from(self.first)
    }
}

/// Parse a selector list (`lxb_css_selectors_parse`). `Err` carries the
/// status: out of memory, or any other error, which is a bad selector.
pub fn parse(input: &[u8]) -> Result<Parsed, Status> {
    let mut p = Parser::new(input)?;
    let comp = Components {
        prelude: St::ComplexList,
        failed: St::Failed,
        end: EndCb::ComponentsEnd,
    };
    if !p.components_push(comp, St::Stub, None, T::Undef) {
        return Err(p.status);
    }
    p.status = p.run();
    match p.status {
        Status::Ok => match p.sel.list {
            Some(first) => Ok(Parsed {
                ast: p.ast,
                first: Some(first),
            }),
            None => Err(Status::ErrorUnexpectedData),
        },
        st => Err(st),
    }
}

/// `lxb_css_parser_t` with its selectors module and memory.
struct Parser<'a> {
    tkz: Tokenizer<'a>,
    rules: Vec<Rule>,
    /// `parser->rules`, an index into `rules`; 0 is Lexbor's stub entry.
    r: usize,
    states: Vec<StateEntry>,
    /// `parser->states`, an index into `states`; 0 is the root entry.
    s: usize,
    /// `types_begin..types_pos`.
    types: Vec<T>,
    status: Status,
    loop_: bool,
    sel: Selectors,
    ast: Ast,
    /// State calls left before the run is refused (see `run`).
    budget: usize,
    /// `parser->fake_null`: a phase asked to be run again.
    fake_null: bool,
    offset: DeclOffset,
    /// The stylesheet's rules (`parser->memory`'s rule objects).
    sheet: Rules,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8]) -> Result<Parser<'a>, Status> {
        let mut rules = Vec::new();
        let mut states = Vec::new();
        if rules.falloc_push(Rule::default()).is_err()
            || states
                .falloc_push(StateEntry {
                    state: St::Stub,
                    context: None,
                    root: true,
                })
                .is_err()
        {
            return Err(Status::ErrorMemoryAllocation);
        }
        Ok(Parser {
            tkz: Tokenizer::new(input),
            rules,
            r: 0,
            states,
            s: 0,
            types: Vec::new(),
            status: Status::Ok,
            loop_: false,
            sel: Selectors {
                list: None,
                list_last: None,
                parent: None,
                combinator: comb::DESCENDANT,
                comb_default: comb::DESCENDANT,
                err_in_function: false,
            },
            ast: Ast::default(),
            budget: input.len().saturating_mul(1000).saturating_add(1 << 16),
            fake_null: false,
            offset: DeclOffset::default(),
            sheet: Rules::default(),
        })
    }

    /* ==== the rule machinery: css/syntax/parser.c ==== */

    /// `lxb_css_syntax_parser_run`.
    ///
    /// Lexbor has no bound on the state calls; this one stops a run that
    /// makes over a thousand calls per input byte, which no run of the
    /// ported states does (each token is read a bounded number of times), so
    /// that a fault in the port is an error rather than a hang.
    fn run(&mut self) -> Status {
        self.loop_ = true;
        loop {
            let Some(token) = self.token() else {
                if self.fake_null {
                    self.fake_null = false;
                    if self.budget == 0 {
                        self.status = Status::ErrorOverflow;
                        return self.status;
                    }
                    self.budget -= 1;
                    if !self.loop_ {
                        break;
                    }
                    continue;
                }
                return self.status;
            };
            loop {
                if self.budget == 0 {
                    self.status = Status::ErrorOverflow;
                    return self.status;
                }
                self.budget -= 1;
                let rule = self.rules[self.r];
                if self.state(rule.state, token, rule.context) {
                    break;
                }
            }
            /* Out of memory ends the run, wherever it was met. Lexbor's
             * skipping states (`while (token != NULL && ...)`) end their loop
             * on a failed read and report success, and a later callback's
             * status (`parser->status = prelude_end(...)`) overwrites the
             * failure, so the run returned a truncated sheet as a success
             * (docs/parser_differences.ja.md S3). */
            if self.status == Status::ErrorMemoryAllocation {
                return self.status;
            }
            if !self.loop_ {
                break;
            }
        }
        self.status
    }

    /// `lxb_css_syntax_parser_token`. On `None` the failure is in
    /// `self.status`.
    ///
    /// Lexbor's callers read `tkz->status` after a failed token instead, which
    /// is OK when the failure was the parser's own (its bracket stack could not
    /// grow): the run then ends as a success over a truncated list. The port's
    /// callers read `self.status`, so an out-of-memory is reported as one.
    fn token(&mut self) -> Option<Token> {
        let Some(token) = self.tkz.token() else {
            self.status = self.tkz.status;
            return None;
        };
        let r = self.r;
        match self.rules[r].phase {
            Phase::Components => self.phase_components(token, r),
            Phase::Function => self.phase_function(token, r),
            Phase::End => self.phase_end(token, r, false),
            Phase::EndConsumeToken => self.phase_end(token, r, true),
            Phase::ListRules => self.phase_list_rules(token, r),
            Phase::ListRulesAt => self.at_begin(token, r, St::ListRulesNext, false),
            Phase::ListRulesQualified => self.phase_list_rules_qualified(r),
            Phase::AtRule => self.phase_at_rule(token, r),
            Phase::AtBlock => self.phase_at_block(token, r),
            Phase::AtPreludeEnd => self.phase_at_prelude_end(token, r, false),
            Phase::AtPreludeEndConsumeToken => self.phase_at_prelude_end(token, r, true),
            Phase::Qualified => self.phase_qualified(token, r),
            Phase::QualifiedBlock => self.phase_qualified_block(token, r),
            Phase::QualifiedPreludeEnd => self.phase_qualified_prelude_end(token, r),
            Phase::Block => self.phase_block(token, r),
            Phase::BlockAt => self.at_begin(token, r, St::BlockNext, true),
            Phase::BlockDeclarations => self.phase_block_declarations(r),
            Phase::BlockQualified => self.phase_block_qualified(r),
            Phase::DeclarationsBegin => self.phase_declarations_begin(token, r),
            Phase::DeclarationsName => self.phase_declarations_name(token, r),
            Phase::DeclarationsValue => self.phase_declarations_value(token, r),
            Phase::DeclarationsNext => self.phase_declarations_next(token, r),
            Phase::DeclarationEnd => self.phase_declaration_end(token, r),
            Phase::DeclarationsEnd => self.phase_declarations_end(token, r),
            Phase::Stub => {
                self.status = Status::ErrorWrongArgs;
                None
            }
        }
    }

    /// `lxb_css_syntax_parser_token_wo_ws`.
    fn token_wo_ws(&mut self) -> Option<Token> {
        let token = self.token()?;
        if token.ty == T::Whitespace {
            self.consume();
            return self.token();
        }
        Some(token)
    }

    /// `lxb_css_syntax_parser_consume`.
    fn consume(&mut self) {
        if !self.rules[self.r].skip_consume {
            self.tkz.consume();
        }
    }

    /// `lxb_css_parser_token_end`.
    fn token_end(offset: usize) -> Token {
        Token {
            ty: T::End,
            offset,
            ..Token::default()
        }
    }

    /// `lxb_css_parser_types_push`; the rule's `deep` is the caller's.
    fn types_push(&mut self, ty: T) -> bool {
        if self.types.falloc_push(ty).is_err() {
            self.status = Status::ErrorMemoryAllocation;
            return false;
        }
        true
    }

    /// `lxb_css_syntax_parser_components`.
    fn phase_components(&mut self, token: Token, r: usize) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        self.rules[r].offset = token.offset + token.length;
        let done =
            (self.rules[r].block_end == token.ty && self.rules[r].deep == 0) || token.ty == T::Eof;
        if !done {
            let closer = match token.ty {
                T::LsBracket => T::RsBracket,
                T::Function | T::LParenthesis => T::RParenthesis,
                T::LcBracket => T::RcBracket,
                T::RcBracket | T::RsBracket | T::RParenthesis => {
                    if self.rules[r].deep != 0 && self.types.last() == Some(&token.ty) {
                        self.types.pop();
                        self.rules[r].deep -= 1;
                    }
                    return Some(token);
                }
                _ => return Some(token),
            };
            if !self.types_push(closer) {
                return None;
            }
            self.rules[r].deep += 1;
            return Some(token);
        }
        self.rules[r].phase = Phase::End;
        self.rules[r].skip_consume = true;
        Some(Self::token_end(token.offset))
    }

    /// `lxb_css_syntax_parser_function`.
    fn phase_function(&mut self, token: Token, r: usize) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        self.rules[r].offset = token.offset + token.length;
        let closer = match token.ty {
            T::LsBracket => T::RsBracket,
            T::Function | T::LParenthesis => T::RParenthesis,
            T::LcBracket => T::RcBracket,
            T::RParenthesis => {
                if self.rules[r].deep == 0 {
                    self.rules[r].phase = Phase::EndConsumeToken;
                    self.rules[r].skip_consume = true;
                    return Some(Self::token_end(token.offset));
                }
                if self.types.last() == Some(&token.ty) {
                    self.types.pop();
                    self.rules[r].deep -= 1;
                }
                return Some(token);
            }
            T::RcBracket | T::RsBracket => {
                if self.rules[r].deep != 0 && self.types.last() == Some(&token.ty) {
                    self.types.pop();
                    self.rules[r].deep -= 1;
                }
                return Some(token);
            }
            T::Eof => {
                self.rules[r].phase = Phase::End;
                self.rules[r].skip_consume = true;
                return Some(Self::token_end(token.offset));
            }
            _ => return Some(token),
        };
        if !self.types_push(closer) {
            return None;
        }
        self.rules[r].deep += 1;
        Some(token)
    }

    /// `lxb_css_syntax_parser_end` and `..._end_consume_token`.
    fn phase_end(&mut self, token: Token, r: usize, consume_token: bool) -> Option<Token> {
        self.rules[r].skip_consume = true;
        if self.rules[r].state != St::Success {
            return Some(Self::token_end(token.offset));
        }
        let rule = self.rules[r];
        let status = self.end_cb(rule);
        if status != Status::Ok {
            self.status = status;
            return None;
        }
        self.rules[r].skip_consume = false;
        if !consume_token {
            return self.call_back(token);
        }
        self.tkz.consume();
        let Some(token) = self.tkz.token() else {
            self.status = self.tkz.status;
            return None;
        };
        self.call_back(token)
    }

    /// `lxb_css_syntax_parser_call_back`.
    fn call_back(&mut self, token: Token) -> Option<Token> {
        self.r = self.r.saturating_sub(1);
        let r = self.r;
        if r == 0 {
            self.rules[0].state = St::Stop;
            return Some(token);
        }
        let rule = &mut self.rules[r];
        let (phase, skip_consume) = match rule.back {
            Back::Components => (Phase::Components, false),
            Back::Function => (Phase::Function, false),
            Back::ListRules => (Phase::ListRules, true),
            Back::AtToPrelude => (Phase::AtRule, false),
            Back::QualifiedToPrelude => (Phase::Qualified, false),
            Back::Block => (Phase::Block, true),
            Back::DeclarationToValue => (Phase::DeclarationsValue, false),
            Back::ToEnd => {
                rule.phase = Phase::End;
                rule.state = St::Blank;
                return Some(token);
            }
            Back::Stub => {
                self.status = Status::ErrorWrongArgs;
                return None;
            }
        };
        rule.phase = phase;
        rule.state = rule.back_state;
        rule.skip_consume = skip_consume;
        Some(token)
    }

    /// The rule push shared by `lxb_css_syntax_parser_components_push` and
    /// `..._function_push`.
    fn push_rule(&mut self, back: St, rule: Rule) -> bool {
        if self.r + 1 >= self.rules.len() && self.rules.falloc_reserve(1).is_err() {
            self.status = Status::ErrorMemoryAllocation;
            return false;
        }
        self.rules[self.r].back_state = back;
        self.r += 1;
        if self.r == self.rules.len() {
            self.rules.push(rule);
        } else {
            self.rules[self.r] = rule;
        }
        true
    }

    /// `lxb_css_syntax_parser_components_push`.
    fn components_push(&mut self, comp: Components, back: St, ctx: Ctx, stop: T) -> bool {
        self.push_rule(
            back,
            Rule {
                phase: Phase::Components,
                state: comp.prelude,
                back: Back::Components,
                context: ctx,
                failed_state: comp.failed,
                end: comp.end,
                block_end: stop,
                ..Rule::default()
            },
        )
    }

    /// `lxb_css_syntax_parser_function_push`.
    fn function_push(&mut self, func: &FuncData, back: St, ctx: Ctx) -> bool {
        self.push_rule(
            back,
            Rule {
                phase: Phase::Function,
                state: func.value,
                back: Back::Function,
                context: ctx,
                failed_state: St::Failed,
                end: func.end,
                ..Rule::default()
            },
        )
    }

    /// `lxb_css_syntax_consume_function`.
    fn consume_function(&mut self, token: Token, func: &FuncData, back: St, ctx: Ctx) -> bool {
        if token.ty != T::Function {
            return false;
        }
        if self.r > 0 && self.rules[self.r].deep != 0 && self.types.last() == Some(&T::RParenthesis)
        {
            self.types.pop();
            self.rules[self.r].deep -= 1;
        }
        self.consume();
        self.function_push(func, back, ctx)
    }

    /* ==== the parser: css/parser.c, css/state.c ==== */

    /// `lxb_css_parser_states_push`.
    fn states_push(&mut self, state: St, ctx: Ctx, root: bool) -> bool {
        let entry = StateEntry {
            state,
            context: ctx,
            root,
        };
        if self.s + 1 == self.states.len() {
            if self.states.falloc_push(entry).is_err() {
                self.status = Status::ErrorMemoryAllocation;
                return false;
            }
        } else {
            self.states[self.s + 1] = entry;
        }
        self.s += 1;
        true
    }

    /// `lxb_css_parser_states_next`.
    fn states_next(&mut self, next: St, back: St, ctx: Ctx, root: bool) -> bool {
        if !self.states_push(back, ctx, root) {
            return false;
        }
        self.rules[self.r].state = next;
        true
    }

    /// `lxb_css_parser_states_pop`.
    fn states_pop(&mut self) {
        self.s = self.s.saturating_sub(1);
    }

    /// `lxb_css_parser_states_to_root`.
    fn states_to_root(&mut self) {
        while self.s > 0 && !self.states[self.s].root {
            self.s -= 1;
        }
    }

    /// `lxb_css_parser_states_set_back`.
    fn states_set_back(&mut self) -> bool {
        let entry = self.states[self.s];
        let rule = &mut self.rules[self.r];
        rule.state = entry.state;
        rule.context = entry.context;
        true
    }

    /// `lxb_css_parser_stop`.
    fn stop(&mut self) -> bool {
        self.loop_ = false;
        true
    }

    /// `lxb_css_parser_fail`.
    fn fail(&mut self, status: Status) -> bool {
        self.status = status;
        self.loop_ = false;
        true
    }

    /// `lxb_css_parser_memory_fail`.
    fn memory_fail(&mut self) -> bool {
        self.fail(Status::ErrorMemoryAllocation)
    }

    /// `lxb_css_parser_unexpected_status`.
    fn unexpected_status(&mut self) -> Status {
        self.status = Status::ErrorUnexpectedData;
        self.rules[self.r].failed = true;
        Status::ErrorUnexpectedData
    }

    /// `lxb_css_parser_unexpected`.
    fn unexpected(&mut self) -> bool {
        self.unexpected_status();
        true
    }

    /// `lxb_css_parser_success`.
    fn success(&mut self) -> bool {
        self.rules[self.r].state = St::Success;
        true
    }

    /// `lxb_css_parser_failed`.
    fn failed(&mut self) -> bool {
        let rule = &mut self.rules[self.r];
        rule.state = rule.failed_state;
        rule.failed = true;
        true
    }

    /// `lxb_css_parser_set_ok`.
    fn set_ok(&mut self) {
        self.rules[self.r].failed = false;
        self.status = Status::Ok;
    }

    /// `lxb_css_parser_failed_set_by_id(parser, -1, true)`.
    fn failed_set_parent(&mut self) {
        if self.r >= 2 {
            self.rules[self.r - 1].failed = true;
        }
    }

    /// Dispatch a state (`rules->state(parser, token, ctx)`).
    fn state(&mut self, st: St, token: Token, ctx: Ctx) -> bool {
        match st {
            St::Stub => self.fail(Status::ErrorWrongArgs),
            St::Success => self.state_success(token),
            St::Failed => self.state_failed(token),
            St::Stop => self.stop(),
            St::ComplexList => self.complex_list(ctx),
            St::ComplexListEnd => self.list_end(token, St::ComplexWoRoot),
            St::RelativeList => self.relative_list(ctx),
            St::RelativeListEnd => self.list_end(token, St::RelativeListWoRoot),
            St::RelativeListWoRoot => self.relative_handler(token, ctx),
            St::ComplexWoRoot => self.complex_wo_root(ctx),
            St::ComplexEnd => self.complex_end(token),
            St::CompoundWoRoot => self.compound_wo_root(),
            St::CompoundHandler => self.compound_handler(token),
            St::CompoundSub => self.compound_sub(token),
            St::CompoundPseudo => self.compound_pseudo(token),
            St::PseudoClassFunctionUndef | St::PseudoClassFunctionUnsupported => {
                self.fail(Status::ErrorUnexpectedData)
            }
            St::PseudoClassFunctionList => self.pseudo_function_list(St::ComplexList),
            St::PseudoClassFunctionHas => self.pseudo_function_list(St::RelativeList),
            St::PseudoAnbBegin => self.pseudo_anb_begin(token),
            St::PseudoOfBegin => self.pseudo_of_begin(token),
            St::PseudoOfBack => self.pseudo_of_back(),
            St::PseudoElementFunctionUndef => true,
            St::Blank => self.success(),
            St::ListRulesNext => self.list_rules_next(),
            St::AtPrelude => self.at_rule_prelude(token),
            St::SkipToEnd => self.skip_to_end(token),
            St::QualifiedPrelude => self.qualified_rule_prelude(token),
            St::BlockNext => self.block_next(),
            St::DeclarationValue => self.declaration_value(token),
            St::DeclarationsBad => self.declarations_bad(token),
        }
    }

    /// Dispatch a rule's end callback (`base->end(parser, token, ctx,
    /// failed)`).
    fn end_cb(&mut self, rule: Rule) -> Status {
        let (ctx, failed) = (rule.context, rule.failed);
        match rule.end {
            EndCb::ComponentsEnd => self.components_end(failed),
            EndCb::FunctionEnd => self.function_end(ctx),
            EndCb::FunctionForgiving | EndCb::FunctionForgivingRelative => self.forgiving_cb(ctx),
            EndCb::PseudoOfEnd => self.pseudo_of_end(ctx),
            EndCb::ListRulesEnd | EndCb::BlockEnd | EndCb::DeclarationsEnd => {
                self.set_return(rule.item);
                Some(Status::Ok)
            }
            EndCb::AtRuleEnd(kind) => self.at_rule_end(kind, rule.item, failed),
            EndCb::QualifiedEnd => self.qualified_rule_end(rule.item, failed),
        }
        .unwrap_or(Status::ErrorWrongArgs)
    }

    /// `lxb_css_state_success`.
    fn state_success(&mut self, token: Token) -> bool {
        match token.ty {
            T::Whitespace => {
                self.consume();
                true
            }
            T::End => true,
            _ => self.failed(),
        }
    }

    /// `lxb_css_state_failed`.
    fn state_failed(&mut self, token: Token) -> bool {
        if token.ty == T::End {
            return self.success();
        }
        /* The lxb_css_syntax_parser_consume(...) locked in this state. */
        self.tkz.consume();
        true
    }

    /* ==== the selector graph: selector.c, selectors.h ==== */

    fn oom<V>(&mut self) -> Option<V> {
        self.status = Status::ErrorMemoryAllocation;
        None
    }

    /// `lxb_css_selector_create(list)`.
    fn selector_create(&mut self, list: ListId) -> Option<SelId> {
        let Ok(id) = SelId::try_from(self.ast.sels.len()) else {
            return self.oom();
        };
        if self
            .ast
            .sels
            .falloc_push(SelNode {
                list,
                ..SelNode::default()
            })
            .is_err()
        {
            return self.oom();
        }
        Some(id)
    }

    /// `lxb_css_selector_list_create`.
    fn list_create(&mut self) -> Option<ListId> {
        let Ok(id) = ListId::try_from(self.ast.lists.len()) else {
            return self.oom();
        };
        if self.ast.lists.falloc_push(ListNode::default()).is_err() {
            return self.oom();
        }
        Some(id)
    }

    /// `lxb_css_selector_append_next`.
    fn selector_append_next(&mut self, dist: SelId, src: SelId) {
        let next = self.ast.sel(dist).next;
        if let Some(n) = next {
            self.ast.sel_mut(n).prev = Some(src);
        }
        let s = self.ast.sel_mut(src);
        s.prev = Some(dist);
        s.next = next;
        self.ast.sel_mut(dist).next = Some(src);
    }

    /// `lxb_css_selector_list_append_next`.
    fn list_append_next(&mut self, dist: ListId, src: ListId) {
        let next = self.ast.list(dist).next;
        if let Some(n) = next {
            self.ast.list_mut(n).prev = Some(src);
        }
        let s = self.ast.list_mut(src);
        s.prev = Some(dist);
        s.next = next;
        self.ast.list_mut(dist).next = Some(src);
    }

    /// `lxb_css_selector_remove`.
    fn selector_remove(&mut self, id: SelId) {
        let n = *self.ast.sel(id);
        if let Some(next) = n.next {
            self.ast.sel_mut(next).prev = n.prev;
        }
        if let Some(prev) = n.prev {
            self.ast.sel_mut(prev).next = n.next;
        }
        let list = self.ast.list_mut(n.list);
        if list.first == Some(id) {
            list.first = n.next;
        }
        if list.last == Some(id) {
            list.last = n.prev;
        }
    }

    /// `lxb_css_selector_list_selectors_remove`.
    fn list_selectors_remove(&mut self, id: ListId) {
        let l = *self.ast.list(id);
        if let Some(next) = l.next {
            self.ast.list_mut(next).prev = l.prev;
        }
        if let Some(prev) = l.prev {
            self.ast.list_mut(prev).next = l.next;
        }
        if self.sel.list == Some(id) {
            self.sel.list = l.next;
        }
        if self.sel.list_last == Some(id) {
            self.sel.list_last = l.prev;
        }
    }

    /// `lxb_css_selectors_state_append`: a new selector at the end of the
    /// last list, taking the pending combinator.
    fn append(&mut self) -> Option<SelId> {
        let Some(list) = self.sel.list_last else {
            self.status = Status::ErrorWrongArgs;
            return None;
        };
        let id = self.selector_create(list)?;
        match self.ast.list(list).last {
            Some(last) => self.selector_append_next(last, id),
            None => self.ast.list_mut(list).first = Some(id),
        }
        self.ast.list_mut(list).last = Some(id);
        self.ast.sel_mut(id).combinator = self.sel.combinator;
        self.sel.combinator = comb::CLOSE;
        Some(id)
    }

    /// `lxb_css_selectors_state_list_append`.
    fn list_append(&mut self) -> Option<ListId> {
        let id = self.list_create()?;
        match self.sel.list_last {
            Some(last) => self.list_append_next(last, id),
            None => self.sel.list = Some(id),
        }
        self.sel.list_last = Some(id);
        self.ast.list_mut(id).parent = self.sel.parent;
        Some(id)
    }

    /// `lxb_css_syntax_token_string_dup`: a copy of a token's string.
    fn dup(&mut self, s: Span) -> Option<Span> {
        let bytes = self.tkz.string(s);
        match store(&mut self.ast.strings, bytes) {
            Some(span) => Some(span),
            None => self.oom(),
        }
    }

    /// A literal string in the AST's store (Lexbor's `"*"` allocations).
    fn literal(&mut self, s: &[u8]) -> Option<Span> {
        match store(&mut self.ast.strings, s) {
            Some(span) => Some(span),
            None => self.oom(),
        }
    }

    fn sp(&self, list: ListId) -> u32 {
        self.ast.list(list).specificity
    }

    fn set_sp(&mut self, list: ListId, sp: u32) {
        self.ast.list_mut(list).specificity = sp;
    }

    /// `lxb_css_selectors_state_specificity_set_b` (`n` 9) and `_set_c`
    /// (`n` 0).
    fn specificity_set_bc(&mut self, n: u32, max: u32) {
        let Some(last) = self.sel.list_last else {
            return;
        };
        let mut sp = self.sp(last);
        if self.sel.parent.is_none() {
            sp = sp_set(sp, n, sp_get(sp, n).wrapping_add(1));
        } else if sp > max {
            if self.sel.combinator == comb::CLOSE {
                sp = sp_set(sp, n, 1);
            }
        } else {
            if self.sel.combinator != comb::CLOSE {
                sp = 0;
            }
            sp = sp_set(sp, n, 1);
        }
        self.set_sp(last, sp);
    }

    fn specificity_set_b(&mut self) {
        self.specificity_set_bc(9, SP_B_MAX);
    }

    fn specificity_set_c(&mut self) {
        self.specificity_set_bc(0, SP_C_MAX);
    }

    /// `lxb_css_selectors_state_func_specificity`.
    fn func_specificity(&mut self) {
        let Some(last) = self.sel.list_last else {
            return;
        };
        let Some(prev) = self.ast.list(last).prev else {
            return;
        };
        if self.sp(prev) > self.sp(last) {
            let sp = self.sp(prev);
            self.set_sp(last, sp);
        }
        self.set_sp(prev, 0);
    }

    /* ==== the selector states: css/selectors/state.c ==== */

    /// `lxb_css_selectors_done`.
    fn done(&mut self) -> bool {
        self.states_pop();
        self.states_set_back()
    }

    /// `lxb_css_selectors_state_complex_list`.
    fn complex_list(&mut self, ctx: Ctx) -> bool {
        if !self.states_next(St::ComplexWoRoot, St::ComplexListEnd, ctx, true) {
            return self.memory_fail();
        }
        false
    }

    /// `lxb_css_selectors_state_relative_list`.
    fn relative_list(&mut self, ctx: Ctx) -> bool {
        if !self.states_next(St::RelativeListWoRoot, St::RelativeListEnd, ctx, true) {
            return self.memory_fail();
        }
        false
    }

    /// `lxb_css_selectors_state_relative_handler` with `list` true and
    /// `root` false (`lxb_css_selectors_state_relative_list_wo_root`).
    fn relative_handler(&mut self, token: Token, ctx: Ctx) -> bool {
        let mut consume = true;
        match token.ty {
            T::Whitespace => {
                self.consume();
                self.sel.combinator = comb::DESCENDANT;
                return true;
            }
            T::Delim => match token.delim {
                0x3E => self.sel.combinator = comb::CHILD,
                0x2B => self.sel.combinator = comb::SIBLING,
                0x7E => self.sel.combinator = comb::FOLLOWING,
                0x7C => {
                    let Some(next) = self.tkz.token_next() else {
                        let st = self.tkz.status;
                        return self.fail(st);
                    };
                    if next.ty == T::Delim && next.delim == 0x7C {
                        self.consume();
                        self.sel.combinator = comb::CELL;
                    } else {
                        consume = false;
                    }
                }
                _ => consume = false,
            },
            _ => consume = false,
        }
        if consume {
            self.consume();
        }
        if !self.states_next(St::CompoundWoRoot, St::ComplexEnd, ctx, false) {
            return self.memory_fail();
        }
        true
    }

    /// `lxb_css_selectors_state_complex_wo_root`.
    fn complex_wo_root(&mut self, ctx: Ctx) -> bool {
        if !self.states_next(St::CompoundWoRoot, St::ComplexEnd, ctx, false) {
            return self.memory_fail();
        }
        false
    }

    /// `lxb_css_selectors_state_complex_end`.
    fn complex_end(&mut self, mut token: Token) -> bool {
        loop {
            match token.ty {
                T::Whitespace => {
                    self.consume();
                    self.sel.combinator = comb::DESCENDANT;
                    let Some(t) = self.token() else {
                        let st = self.status;
                        return self.fail(st);
                    };
                    token = t;
                    continue;
                }
                T::End | T::Comma => return self.done(),
                T::Delim => {
                    match token.delim {
                        0x3E => self.sel.combinator = comb::CHILD,
                        0x2B => self.sel.combinator = comb::SIBLING,
                        0x7E => self.sel.combinator = comb::FOLLOWING,
                        0x7C => {
                            let Some(next) = self.tkz.token_next() else {
                                let st = self.tkz.status;
                                return self.fail(st);
                            };
                            if next.ty == T::Delim && next.delim == 0x7C {
                                self.consume();
                                self.sel.combinator = comb::CELL;
                            } else {
                                break;
                            }
                        }
                        _ => {
                            if self.sel.combinator != comb::DESCENDANT {
                                self.done();
                                return self.unexpected();
                            }
                            break;
                        }
                    }
                    self.consume();
                    break;
                }
                _ => {
                    if self.sel.combinator != comb::DESCENDANT {
                        self.done();
                        return self.unexpected();
                    }
                    break;
                }
            }
        }
        self.rules[self.r].state = St::CompoundHandler;
        true
    }

    /// `lxb_css_selectors_state_compound_wo_root`.
    fn compound_wo_root(&mut self) -> bool {
        if self.list_append().is_none() {
            return self.memory_fail();
        }
        self.rules[self.r].state = St::CompoundHandler;
        false
    }

    /// The pseudo-class / pseudo-element dispatch after a `:`, shared by
    /// `lxb_css_selectors_state_compound_handler` and `..._sub`. `Err`
    /// is the handler's own early `return lxb_css_parser_unexpected(parser)`.
    fn colon(&mut self, after_class_function: St) -> Result<Status, bool> {
        self.consume();
        let Some(token) = self.token() else {
            let st = self.status;
            return Err(self.fail(st));
        };
        if token.ty == T::Ident {
            return Ok(self.pseudo_class(token));
        }
        if token.ty == T::Colon {
            self.consume();
            let Some(token) = self.token() else {
                let st = self.status;
                return Err(self.fail(st));
            };
            if token.ty == T::Ident {
                self.rules[self.r].state = St::CompoundPseudo;
                return Ok(self.pseudo_element(token));
            }
            if token.ty != T::Function {
                return Err(self.unexpected());
            }
            return Ok(self.pseudo_element_function(token, St::CompoundPseudo));
        }
        if token.ty != T::Function {
            return Ok(Status::ErrorUnexpectedData);
        }
        Ok(self.pseudo_class_function(token, after_class_function))
    }

    /// `lxb_css_selectors_state_compound_handler`.
    fn compound_handler(&mut self, mut token: Token) -> bool {
        let status = loop {
            self.rules[self.r].state = St::CompoundSub;
            match token.ty {
                T::Hash => break self.selector_hash(token),
                T::Delim => match token.delim {
                    0x2E => {
                        self.consume();
                        break self.selector_class();
                    }
                    0x7C | 0x2A => break self.element_ns(token),
                    _ => break Status::ErrorUnexpectedData,
                },
                T::Ident => break self.element(token),
                T::LsBracket => {
                    self.consume();
                    break self.attribute();
                }
                T::Colon => match self.colon(St::CompoundSub) {
                    Ok(st) => break st,
                    Err(ret) => return ret,
                },
                T::Whitespace => {
                    self.consume();
                    let Some(t) = self.token() else {
                        let st = self.status;
                        return self.fail(st);
                    };
                    token = t;
                }
                T::End => {
                    let empty = self
                        .sel
                        .list_last
                        .is_none_or(|l| self.ast.list(l).first.is_none());
                    if self.sel.combinator > comb::CLOSE || empty {
                        break Status::ErrorUnexpectedData;
                    }
                    return self.done();
                }
                _ => break Status::ErrorUnexpectedData,
            }
        };
        match status {
            Status::Ok => true,
            Status::ErrorMemoryAllocation => self.memory_fail(),
            _ => {
                self.states_to_root();
                self.states_set_back();
                self.unexpected()
            }
        }
    }

    /// `lxb_css_selectors_state_compound_sub`.
    fn compound_sub(&mut self, token: Token) -> bool {
        let status = match token.ty {
            T::Hash => self.selector_hash(token),
            T::Delim => {
                if token.delim != 0x2E {
                    return self.states_set_back();
                }
                self.consume();
                self.selector_class()
            }
            T::LsBracket => {
                self.consume();
                self.attribute()
            }
            T::Colon => match self.colon(St::CompoundSub) {
                Ok(Status::ErrorUnexpectedData) => return self.unexpected(),
                Ok(st) => st,
                Err(ret) => return ret,
            },
            _ => return self.states_set_back(),
        };
        match status {
            Status::Ok => true,
            Status::ErrorMemoryAllocation => self.memory_fail(),
            _ => self.unexpected(),
        }
    }

    /// `lxb_css_selectors_state_compound_pseudo`.
    fn compound_pseudo(&mut self, token: Token) -> bool {
        if token.ty != T::Colon {
            return self.states_set_back();
        }
        self.consume();
        let Some(token) = self.token() else {
            let st = self.status;
            return self.fail(st);
        };
        let status = if token.ty == T::Ident {
            self.pseudo_class(token)
        } else if token.ty == T::Colon {
            self.consume();
            let Some(token) = self.token() else {
                let st = self.status;
                return self.fail(st);
            };
            if token.ty == T::Ident {
                self.pseudo_element(token)
            } else if token.ty == T::Function {
                self.pseudo_element_function(token, St::CompoundPseudo)
            } else {
                return self.unexpected();
            }
        } else if token.ty != T::Function {
            return self.unexpected();
        } else {
            self.pseudo_class_function(token, St::CompoundPseudo)
        };
        match status {
            Status::Ok => true,
            Status::ErrorMemoryAllocation => self.memory_fail(),
            _ => self.unexpected(),
        }
    }

    /// The `status` a selector state returns when an allocation failed.
    fn oom_status(&self) -> Status {
        if self.status == Status::ErrorMemoryAllocation {
            Status::ErrorMemoryAllocation
        } else {
            Status::ErrorWrongArgs
        }
    }

    /// `lxb_css_selectors_state_hash`.
    fn selector_hash(&mut self, token: Token) -> Status {
        if let Some(last) = self.sel.list_last {
            let mut sp = self.sp(last);
            if self.sel.parent.is_none() {
                sp = sp_set(sp, 18, sp_get(sp, 18).wrapping_add(1));
            } else if sp_get(sp, 18) == 0 {
                if self.sel.combinator != comb::CLOSE {
                    sp = 0;
                }
                sp = sp_set(sp, 18, 1);
            }
            self.set_sp(last, sp);
        }
        let Some(id) = self.append() else {
            return self.oom_status();
        };
        self.ast.sel_mut(id).ty = sel_type::ID;
        let Some(name) = self.dup(token.string) else {
            return Status::ErrorMemoryAllocation;
        };
        self.ast.sel_mut(id).name = Some(name);
        self.consume();
        Status::Ok
    }

    /// `lxb_css_selectors_state_class`.
    fn selector_class(&mut self) -> Status {
        let Some(token) = self.token() else {
            return self.status;
        };
        if token.ty != T::Ident {
            return self.unexpected_status();
        }
        self.specificity_set_b();
        let Some(id) = self.append() else {
            return self.oom_status();
        };
        self.ast.sel_mut(id).ty = sel_type::CLASS;
        let Some(name) = self.dup(token.string) else {
            return Status::ErrorMemoryAllocation;
        };
        self.ast.sel_mut(id).name = Some(name);
        self.consume();
        Status::Ok
    }

    /// `lxb_css_selectors_state_element_ns`.
    fn element_ns(&mut self, token: Token) -> Status {
        let Some(id) = self.append() else {
            return self.oom_status();
        };
        self.ast.sel_mut(id).ty = sel_type::ANY;
        if token.delim == 0x2A {
            let Some(star) = self.literal(b"*") else {
                return Status::ErrorMemoryAllocation;
            };
            self.ast.sel_mut(id).name = Some(star);
            self.consume();
            return self.selector_ns(id);
        }
        /* Lexbor allocates "*" and truncates it to the empty string. */
        let Some(empty) = self.literal(b"") else {
            return Status::ErrorMemoryAllocation;
        };
        self.ast.sel_mut(id).name = Some(empty);
        self.consume();
        self.selector_ns_ident(id)
    }

    /// `lxb_css_selectors_state_element`.
    fn element(&mut self, token: Token) -> Status {
        self.specificity_set_c();
        let Some(id) = self.append() else {
            return self.oom_status();
        };
        self.ast.sel_mut(id).ty = sel_type::ELEMENT;
        let Some(name) = self.dup(token.string) else {
            return Status::ErrorMemoryAllocation;
        };
        self.ast.sel_mut(id).name = Some(name);
        self.consume();
        self.selector_ns(id)
    }

    /// `lxb_css_selectors_state_ns`.
    fn selector_ns(&mut self, id: SelId) -> Status {
        let Some(token) = self.token() else {
            return self.status;
        };
        if token.ty == T::Delim && token.delim == 0x7C {
            self.consume();
            return self.selector_ns_ident(id);
        }
        Status::Ok
    }

    /// `lxb_css_selectors_state_ns_ident`.
    fn selector_ns_ident(&mut self, id: SelId) -> Status {
        let Some(token) = self.token() else {
            return self.status;
        };
        if token.ty == T::Ident {
            self.specificity_set_c();
            let s = self.ast.sel_mut(id);
            s.ty = sel_type::ELEMENT;
            s.ns = s.name.take();
            let Some(name) = self.dup(token.string) else {
                return Status::ErrorMemoryAllocation;
            };
            self.ast.sel_mut(id).name = Some(name);
            self.consume();
            return Status::Ok;
        }
        if token.ty == T::Delim && token.delim == 0x2A {
            self.consume();
            let s = self.ast.sel_mut(id);
            s.ty = sel_type::ANY;
            s.ns = s.name;
            let Some(star) = self.literal(b"*") else {
                return Status::ErrorMemoryAllocation;
            };
            self.ast.sel_mut(id).name = Some(star);
            return Status::Ok;
        }
        self.unexpected_status()
    }

    /// `lxb_css_selectors_state_attribute`.
    fn attribute(&mut self) -> Status {
        /// The labels of Lexbor's function.
        enum Go {
            Op,
            Assignment,
            StringOrIdent,
            Done,
            DoneEof,
            Failed,
        }

        let Some(id) = self.append() else {
            return self.oom_status();
        };
        macro_rules! next {
            () => {
                match self.token() {
                    Some(t) => t,
                    None => return self.status,
                }
            };
        }
        macro_rules! next_wo_ws {
            () => {
                match self.token_wo_ws() {
                    Some(t) => t,
                    None => return self.status,
                }
            };
        }
        macro_rules! dup {
            ($s:expr) => {
                match self.dup($s) {
                    Some(s) => s,
                    None => return Status::ErrorMemoryAllocation,
                }
            };
        }

        let mut token = next_wo_ws!();
        let mut go = match token.ty {
            T::Delim => {
                if token.delim != 0x7C {
                    Go::Failed
                } else {
                    self.consume();
                    token = next!();
                    if token.ty != T::Ident {
                        Go::Failed
                    } else {
                        self.ast.sel_mut(id).ty = sel_type::ATTRIBUTE;
                        let Some(star) = self.literal(b"*") else {
                            return Status::ErrorMemoryAllocation;
                        };
                        self.ast.sel_mut(id).ns = Some(star);
                        let name = dup!(token.string);
                        self.ast.sel_mut(id).name = Some(name);
                        self.consume();
                        token = next_wo_ws!();
                        Go::Op
                    }
                }
            }
            T::Ident => {
                self.ast.sel_mut(id).ty = sel_type::ATTRIBUTE;
                let name = dup!(token.string);
                self.ast.sel_mut(id).name = Some(name);
                self.consume();
                token = next!();
                if !(token.ty == T::Delim && token.delim == 0x7C) {
                    if token.ty == T::Whitespace {
                        self.consume();
                        token = next!();
                    }
                    Go::Op
                } else {
                    self.consume();
                    token = next!();
                    if token.ty != T::Ident {
                        self.ast.sel_mut(id).attr_match = matching::DASH;
                        Go::Assignment
                    } else {
                        let s = self.ast.sel_mut(id);
                        s.ns = s.name.take();
                        let name = dup!(token.string);
                        self.ast.sel_mut(id).name = Some(name);
                        self.consume();
                        token = next_wo_ws!();
                        Go::Op
                    }
                }
            }
            _ => Go::Failed,
        };

        loop {
            go = match go {
                Go::Op => match token.ty {
                    T::RsBracket => Go::Done,
                    T::Delim => {
                        let m = match token.delim {
                            0x7E => Some(matching::INCLUDE),
                            0x7C => Some(matching::DASH),
                            0x5E => Some(matching::PREFIX),
                            0x24 => Some(matching::SUFFIX),
                            0x2A => Some(matching::SUBSTRING),
                            0x3D => None,
                            _ => return self.unexpected_status(),
                        };
                        match m {
                            Some(m) => {
                                self.ast.sel_mut(id).attr_match = m;
                                self.consume();
                                token = next!();
                                Go::Assignment
                            }
                            None => {
                                self.ast.sel_mut(id).attr_match = matching::EQUAL;
                                self.consume();
                                token = next_wo_ws!();
                                Go::StringOrIdent
                            }
                        }
                    }
                    T::End => Go::DoneEof,
                    _ => Go::Failed,
                },
                Go::Assignment => {
                    if !(token.ty == T::Delim && token.delim == 0x3D) {
                        Go::Failed
                    } else {
                        self.consume();
                        token = next_wo_ws!();
                        Go::StringOrIdent
                    }
                }
                Go::StringOrIdent => {
                    if token.ty != T::String && token.ty != T::Ident {
                        Go::Failed
                    } else {
                        let value = dup!(token.string);
                        self.ast.sel_mut(id).value = Some(value);
                        self.consume();
                        token = next_wo_ws!();
                        match token.ty {
                            T::RsBracket => Go::Done,
                            T::End => Go::DoneEof,
                            T::Ident if token.string.len == 1 => {
                                let m = self.tkz.string(token.string).first().copied().unwrap_or(0);
                                let modifier = match m {
                                    b'i' | b'I' => Some(modifier::I),
                                    b's' | b'S' => Some(modifier::S),
                                    _ => None,
                                };
                                match modifier {
                                    None => Go::Failed,
                                    Some(md) => {
                                        self.ast.sel_mut(id).modifier = md;
                                        self.consume();
                                        token = next_wo_ws!();
                                        match token.ty {
                                            T::RsBracket => Go::Done,
                                            T::End => Go::DoneEof,
                                            _ => Go::Failed,
                                        }
                                    }
                                }
                            }
                            _ => Go::Failed,
                        }
                    }
                }
                Go::Done => {
                    self.specificity_set_b();
                    self.consume();
                    return Status::Ok;
                }
                Go::DoneEof => {
                    self.specificity_set_b();
                    return Status::Ok;
                }
                Go::Failed => return self.unexpected_status(),
            };
        }
    }

    /// The selector a pseudo-class or pseudo-element state appends, with the
    /// token's name.
    fn append_named(&mut self, ty: u8, token: Token) -> Result<SelId, Status> {
        let Some(id) = self.append() else {
            return Err(self.oom_status());
        };
        self.ast.sel_mut(id).ty = ty;
        let Some(name) = self.dup(token.string) else {
            return Err(Status::ErrorMemoryAllocation);
        };
        self.ast.sel_mut(id).name = Some(name);
        Ok(id)
    }

    /// `lxb_css_selectors_state_pseudo_class`.
    fn pseudo_class(&mut self, token: Token) -> Status {
        let id = match self.append_named(sel_type::PSEUDO_CLASS, token) {
            Ok(id) => id,
            Err(st) => return st,
        };
        let Some(pseudo) = pseudo::class_by_name(self.tkz.string(token.string)) else {
            return self.unexpected_status();
        };
        if matches!(
            pseudo.id,
            class::CURRENT
                | class::DEFAULT
                | class::FOCUS_VISIBLE
                | class::FOCUS_WITHIN
                | class::FULLSCREEN
                | class::FUTURE
                | class::IN_RANGE
                | class::INDETERMINATE
                | class::INVALID
                | class::LOCAL_LINK
                | class::OUT_OF_RANGE
                | class::PAST
                | class::SCOPE
                | class::TARGET
                | class::TARGET_WITHIN
                | class::USER_INVALID
                | class::VALID
                | class::VISITED
                | class::WARNING
        ) {
            return self.unexpected_status();
        }
        let s = self.ast.sel_mut(id);
        s.pseudo_type = pseudo.id;
        s.data = PseudoData::None;
        self.consume();
        Status::Ok
    }

    /// `lxb_css_selectors_state_pseudo_class_function`.
    fn pseudo_class_function(&mut self, token: Token, success: St) -> Status {
        let id = match self.append_named(sel_type::PSEUDO_CLASS_FUNCTION, token) {
            Ok(id) => id,
            Err(st) => return st,
        };
        let Some(func) = pseudo::class_function_by_name(self.tkz.string(token.string)) else {
            return self.unexpected_status();
        };
        match func.id {
            class_function::DIR
            | class_function::LANG
            | class_function::NTH_COL
            | class_function::NTH_LAST_COL => return self.unexpected_status(),
            class_function::NTH_CHILD | class_function::NTH_LAST_CHILD => self.specificity_set_b(),
            _ => {}
        }
        self.start_function(id, func, token, success)
    }

    /// The common tail of `lxb_css_selectors_state_pseudo_class_function` and
    /// `..._pseudo_element_function`: enter the function's argument.
    fn start_function(
        &mut self,
        id: SelId,
        func: &'static FuncData,
        token: Token,
        success: St,
    ) -> Status {
        let s = self.ast.sel_mut(id);
        s.pseudo_type = func.id;
        s.data = PseudoData::None;
        self.sel.combinator = func.combinator;
        self.sel.comb_default = func.combinator;
        self.sel.parent = Some(id);
        let list_last = self.sel.list_last;
        if !self.consume_function(token, func, success, list_last) {
            self.memory_fail();
            return self.status;
        }
        self.consume();
        Status::Ok
    }

    /// `lxb_css_selectors_state_pseudo_element`.
    fn pseudo_element(&mut self, token: Token) -> Status {
        let id = match self.append_named(sel_type::PSEUDO_ELEMENT, token) {
            Ok(id) => id,
            Err(st) => return st,
        };
        let Some(pseudo) = pseudo::element_by_name(self.tkz.string(token.string)) else {
            return self.unexpected_status();
        };
        if matches!(
            pseudo.id,
            element::AFTER
                | element::BACKDROP
                | element::BEFORE
                | element::FIRST_LETTER
                | element::FIRST_LINE
                | element::GRAMMAR_ERROR
                | element::INACTIVE_SELECTION
                | element::MARKER
                | element::PLACEHOLDER
                | element::SELECTION
                | element::SPELLING_ERROR
                | element::TARGET_TEXT
        ) {
            return self.unexpected_status();
        }
        let s = self.ast.sel_mut(id);
        s.pseudo_type = pseudo.id;
        s.data = PseudoData::None;
        self.consume();
        Status::Ok
    }

    /// `lxb_css_selectors_state_pseudo_element_function`.
    fn pseudo_element_function(&mut self, token: Token, success: St) -> Status {
        let id = match self.append_named(sel_type::PSEUDO_ELEMENT_FUNCTION, token) {
            Ok(id) => id,
            Err(st) => return st,
        };
        let Some(func) = pseudo::element_function_by_name(self.tkz.string(token.string)) else {
            return self.unexpected_status();
        };
        self.start_function(id, func, token, success)
    }

    /// `lxb_css_selectors_state_restore_combinator`.
    fn restore_combinator(&mut self) {
        let mut comb_default = comb::DESCENDANT;
        if let Some(parent) = self.sel.parent {
            let p = self.ast.sel(parent);
            let is_class = p.ty == sel_type::PSEUDO_CLASS_FUNCTION;
            comb_default = pseudo::function_by_id(p.pseudo_type, is_class).combinator;
        }
        self.sel.combinator = comb::CLOSE;
        self.sel.comb_default = comb_default;
    }

    /// `lxb_css_selectors_state_restore_parent`.
    fn restore_parent(&mut self, last: ListId) {
        if let Some(ll) = self.sel.list_last {
            if ll != last {
                let mut dst = self.sp(last);
                let src = self.sp(ll);
                self.sel.list_last = None;
                if self.ast.list(last).parent.is_none() {
                    dst = sp_set(dst, 18, sp_get(dst, 18).wrapping_add(sp_get(src, 18)));
                    dst = sp_set(dst, 9, sp_get(dst, 9).wrapping_add(sp_get(src, 9)));
                    dst = sp_set(dst, 0, sp_get(dst, 0).wrapping_add(sp_get(src, 0)));
                } else if self.sel.combinator == comb::CLOSE {
                    dst |= src;
                } else if src > dst {
                    dst = src;
                }
                self.set_sp(last, dst);
            }
        }
        if let Some(list) = self.sel.list {
            if let Some(f) = self.ast.list(last).last {
                self.ast.sel_mut(f).data = PseudoData::List(list);
            }
        }
        self.sel.list_last = Some(last);
        let mut first = last;
        while let Some(prev) = self.ast.list(first).prev {
            first = prev;
        }
        self.sel.list = Some(first);
        self.sel.parent = self.ast.list(first).parent;
        self.restore_combinator();
    }

    /// The `empty:` tail of `lxb_css_selectors_state_function_end` and
    /// `..._forgiving_cb`: a function with nothing in it.
    fn function_empty(&mut self, set_ok: bool) -> Option<Status> {
        let last = self.sel.list_last?;
        let selector = self.ast.list(last).last?;
        let s = self.ast.sel(selector);
        let is_class = s.ty == sel_type::PSEUDO_CLASS_FUNCTION;
        if pseudo::function_can_empty(s.pseudo_type, is_class) {
            if set_ok {
                self.set_ok();
            }
            return Some(Status::Ok);
        }
        self.selector_remove(selector);
        self.failed_set_parent();
        self.sel.err_in_function = true;
        Some(Status::Ok)
    }

    /// `lxb_css_selectors_state_function_end`.
    fn function_end(&mut self, ctx: Ctx) -> Option<Status> {
        let last = ctx?;
        if self.sel.list_last.is_none() {
            self.restore_parent(last);
            return self.function_empty(true);
        }
        self.restore_parent(last);
        Some(Status::Ok)
    }

    /// `lxb_css_selectors_state_forgiving_cb`.
    fn forgiving_cb(&mut self, ctx: Ctx) -> Option<Status> {
        self.set_ok();
        let last = ctx?;
        let Some(ll) = self.sel.list_last else {
            self.restore_parent(last);
            return self.function_empty(false);
        };
        let parent = self.sel.parent?;
        if self.ast.sel(parent).pseudo_type == class_function::WHERE {
            self.set_sp(ll, 0);
        }
        self.restore_parent(last);
        Some(Status::Ok)
    }

    /// `lxb_css_selectors_components_end`.
    fn components_end(&mut self, failed: bool) -> Option<Status> {
        if failed {
            if let Some(list) = self.sel.list_last {
                self.list_selectors_remove(list);
            }
        }
        Some(Status::Ok)
    }

    /// `lxb_css_selectors_state_list_end`.
    fn list_end(&mut self, mut token: Token, state: St) -> bool {
        if self.rules[self.r].failed {
            match self.function_error(token) {
                Some(t) => token = t,
                None => return self.fail(Status::ErrorMemoryAllocation),
            }
        } else if token.ty == T::Whitespace {
            self.consume();
            match self.token() {
                Some(t) => token = t,
                None => {
                    let st = self.status;
                    return self.fail(st);
                }
            }
        }

        if self.sel.parent.is_some() {
            if let Some(ll) = self.sel.list_last {
                if self.ast.list(ll).prev.is_some() {
                    self.func_specificity();
                }
            }
        }

        if token.ty != T::Comma {
            if self.states[self.s].root {
                if token.ty != T::End && self.function_error(token).is_none() {
                    return self.fail(Status::ErrorMemoryAllocation);
                }
                self.states_pop();
                return self.success();
            }
            return self.done();
        }

        self.sel.combinator = self.sel.comb_default;
        self.tkz.consume();
        self.rules[self.r].state = state;
        self.set_ok();
        true
    }

    /// `lxb_css_selectors_state_function_error`: drop the selector list that
    /// failed and skip to where the parse can go on. `None` is Lexbor's NULL.
    fn function_error(&mut self, token: Token) -> Option<Token> {
        let mut forgiving = false;
        let mut comma = true;
        let list = self.sel.list_last;
        if let Some(parent) = self.sel.parent {
            let p = self.ast.sel(parent);
            let is_class = p.ty == sel_type::PSEUDO_CLASS_FUNCTION;
            let func = pseudo::function_by_id(p.pseudo_type, is_class);
            forgiving = func.forgiving;
            comma = func.comma;
        }
        if !self.sel.err_in_function {
            /* Lexbor reads the token here for its log, which can read one. */
            self.tkz.token()?;
        }
        self.sel.err_in_function = false;

        let mut token = Some(token);
        if forgiving {
            if let Some(list) = list {
                self.list_selectors_remove(list);
            }
            while let Some(t) = token {
                if t.ty == T::End || (comma && t.ty == T::Comma && self.rules[self.r].deep == 0) {
                    break;
                }
                self.consume();
                token = self.token();
            }
            return token;
        }

        self.sel.list = None;
        self.sel.list_last = None;
        while let Some(t) = token {
            if t.ty == T::End {
                break;
            }
            self.consume();
            token = self.token();
        }
        token
    }

    /* ==== css/selectors/pseudo_state.c ==== */

    /// `lxb_css_selectors_state_pseudo_class_function_is` and its twins: the
    /// argument is a selector list read by `prelude`.
    fn pseudo_function_list(&mut self, prelude: St) -> bool {
        self.rules[self.r].state = prelude;
        self.sel.list = None;
        self.sel.list_last = None;
        true
    }

    /// `lxb_css_selectors_state_pseudo_anb`: reads `An+B` into the last
    /// selector.
    fn pseudo_anb(&mut self, token: Token) -> bool {
        let mut anb = Anb::default();
        self.status = self.anb_handler(token, &mut anb);
        if self.status != Status::Ok {
            return true;
        }
        if let Some(list) = self.sel.list_last {
            if let Some(last) = self.ast.list(list).last {
                self.ast.sel_mut(last).data = PseudoData::AnbOf(AnbOf {
                    a: anb.a,
                    b: anb.b,
                    of: None,
                });
            }
        }
        true
    }

    /// `lxb_css_selectors_state_pseudo_anb_begin`.
    fn pseudo_anb_begin(&mut self, token: Token) -> bool {
        self.pseudo_anb(token);
        if self.status != Status::Ok {
            self.sel.list = None;
            self.sel.list_last = None;
            return self.failed();
        }
        self.sel.list = None;
        self.success()
    }

    /// `lxb_css_selectors_state_pseudo_of_begin`.
    fn pseudo_of_begin(&mut self, token: Token) -> bool {
        self.pseudo_anb(token);
        if self.status != Status::Ok {
            self.sel.list = None;
            self.sel.list_last = None;
            if self.token().is_none() {
                return self.memory_fail();
            }
            return self.failed();
        }
        let list = self.sel.list_last;
        self.sel.list = None;
        let Some(token) = self.token_wo_ws() else {
            let st = self.status;
            return self.fail(st);
        };
        if token.ty == T::Ident && self.tkz.string(token.string).eq_ignore_ascii_case(b"of") {
            self.tkz.consume();
            self.sel.list = None;
            self.sel.list_last = None;
            if self.token().is_none() {
                return self.memory_fail();
            }
            let comp = Components {
                prelude: St::ComplexList,
                failed: St::Failed,
                end: EndCb::PseudoOfEnd,
            };
            if !self.components_push(comp, St::PseudoOfBack, list, T::RParenthesis) {
                return self.memory_fail();
            }
            self.rules[self.r].state = St::ComplexList;
            return true;
        }
        self.success()
    }

    /// `lxb_css_selectors_state_pseudo_of_end`.
    fn pseudo_of_end(&mut self, ctx: Ctx) -> Option<Status> {
        let list = ctx?;
        let last = self.ast.list(list).last?;
        let of = self.sel.list;
        if let PseudoData::AnbOf(anbof) = &mut self.ast.sel_mut(last).data {
            anbof.of = of;
        }
        self.sel.list = None;
        Some(Status::Ok)
    }

    /// `lxb_css_selectors_state_pseudo_of_back`.
    fn pseudo_of_back(&mut self) -> bool {
        self.sel.list = None;
        self.success()
    }

    /* ==== css/syntax/anb.c ==== */

    /// `lxb_css_syntax_anb_handler`.
    fn anb_handler(&mut self, mut token: Token, anb: &mut Anb) -> Status {
        loop {
            match token.ty {
                T::Dimension => {
                    if token.is_float {
                        return Status::ErrorUnexpectedData;
                    }
                    anb.a = double_to_long(token.num);
                    return self.anb_ident(token, anb);
                }
                T::Ident => return self.anb_state_ident(token, anb),
                T::Number => {
                    if token.is_float {
                        return Status::ErrorUnexpectedData;
                    }
                    anb.a = 0;
                    anb.b = double_to_long(token.num);
                    self.consume();
                    return Status::Ok;
                }
                T::Delim => {
                    if token.delim != 0x2B {
                        return Status::ErrorUnexpectedData;
                    }
                    self.consume();
                    let Some(t) = self.token() else {
                        return self.status;
                    };
                    if t.ty != T::Ident {
                        return Status::ErrorUnexpectedData;
                    }
                    anb.a = 1;
                    return self.anb_ident(t, anb);
                }
                T::Whitespace => {
                    self.consume();
                    let Some(t) = self.token() else {
                        return self.status;
                    };
                    token = t;
                }
                _ => return Status::ErrorUnexpectedData,
            }
        }
    }

    /// The `ident:` label of `lxb_css_syntax_anb_handler`: the string must
    /// start with `n`.
    fn anb_ident(&mut self, token: Token, anb: &mut Anb) -> Status {
        let s = self.tkz.string(token.string);
        if !matches!(s.first(), Some(b'n' | b'N')) {
            return Status::ErrorUnexpectedData;
        }
        self.anb_state_ident_data(anb, token.string, 1)
    }

    /// `lxb_css_syntax_anb_state_ident`.
    fn anb_state_ident(&mut self, token: Token, anb: &mut Anb) -> Status {
        let s = self.tkz.string(token.string);
        let mut data = 1;
        match s.first() {
            Some(b'n' | b'N') => anb.a = 1,
            Some(b'-') => {
                if s.len() <= 1 || !matches!(s[1], b'n' | b'N') {
                    return Status::ErrorUnexpectedData;
                }
                data = 2;
                anb.a = -1;
            }
            _ if s.eq_ignore_ascii_case(b"odd") => {
                anb.a = 2;
                anb.b = 1;
                self.consume();
                return Status::Ok;
            }
            _ if s.eq_ignore_ascii_case(b"even") => {
                anb.a = 2;
                anb.b = 0;
                self.consume();
                return Status::Ok;
            }
            _ => return Status::ErrorUnexpectedData,
        }
        self.anb_state_ident_data(anb, token.string, data)
    }

    /// `lxb_css_syntax_anb_state_ident_data`: what follows the `n` of the
    /// string `s`, from its byte `from`.
    fn anb_state_ident_data(&mut self, anb: &mut Anb, s: Span, from: usize) -> Status {
        macro_rules! next_wo_ws {
            () => {
                match self.token_wo_ws() {
                    Some(t) => t,
                    None => return self.status,
                }
            };
        }
        let bytes = self.tkz.string(s);
        let rest = bytes.get(from..).unwrap_or(&[]);
        let mut sign = 0;
        let token;
        if rest.is_empty() {
            self.consume();
            let t = next_wo_ws!();
            match t.ty {
                T::Number => {
                    if !t.have_sign {
                        anb.b = 0;
                        return Status::Ok;
                    }
                    token = t;
                }
                T::Delim => {
                    match t.delim {
                        0x2D => sign = 1,
                        0x2B => sign = 2,
                        _ => {
                            anb.b = 0;
                            return Status::Ok;
                        }
                    }
                    self.consume();
                    token = next_wo_ws!();
                }
                _ => {
                    anb.b = 0;
                    return Status::Ok;
                }
            }
        } else {
            if rest[0] != b'-' {
                return Status::ErrorUnexpectedData;
            }
            let digits = &rest[1..];
            if !digits.is_empty() {
                let (value, read) = data_to_long(digits);
                anb.b = value.wrapping_neg();
                if anb.b > 0 || read == 0 || read < digits.len() {
                    return Status::ErrorUnexpectedData;
                }
                self.consume();
                return Status::Ok;
            }
            sign = 1;
            self.consume();
            token = next_wo_ws!();
        }

        if token.ty != T::Number {
            return Status::ErrorUnexpectedData;
        }
        if token.is_float || (sign > 0 && token.have_sign) {
            return Status::ErrorUnexpectedData;
        }
        anb.b = double_to_long(token.num);
        if sign == 1 {
            anb.b = anb.b.wrapping_neg();
        }
        self.consume();
        Status::Ok
    }
}

/// Append `bytes` to the AST's string store.
fn store(strings: &mut Vec<u8>, bytes: &[u8]) -> Option<Span> {
    let start = u32::try_from(strings.len()).ok()?;
    let len = u32::try_from(bytes.len()).ok()?;
    strings.falloc_reserve(bytes.len()).ok()?;
    strings.extend_from_slice(bytes);
    Some(Span { start, len })
}
