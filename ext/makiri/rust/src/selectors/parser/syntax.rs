//! The stylesheet half of the rule machinery: the rule-list, at-rule,
//! qualified-rule, block and declaration phases of `css/syntax/parser.c`,
//! the callbacks of `css/state.c` that build the rules, and the at-rule
//! callbacks of `css/at_rule/state.c` - in the same shape as the component
//! and function phases in the parent module.
//!
//! Two things are left out, both unreachable from a stylesheet: the
//! declaration list's name validation (`..._declarations_validate`) and its
//! drop phase (`..._declarations_drop`), which only a declaration list
//! parsed on its own (`lxb_css_syntax_parse_declarations`) uses - a block
//! pushes its declarations nested and unvalidated. And a declaration's value
//! is read by one state for every property ([`St::DeclarationValue`]).

use super::*;
use crate::selectors::rules::{Item, Node};

/// A parsed stylesheet: its rules, and the selectors and names they refer
/// to.
pub struct Sheet {
    pub ast: Ast,
    pub rules: Rules,
    /// The top-level rule list.
    pub root: ItemId,
}

/// Parse a stylesheet (`lxb_css_stylesheet_parse`). `Err` is a status Lexbor
/// would have returned: out of memory, or a selector state that stops the
/// whole parse.
pub fn parse_stylesheet(input: &[u8]) -> Result<Sheet, Status> {
    let mut p = Parser::new(input)?;
    let Some(root) = p.item_create(Item::List {
        first: None,
        last: None,
    }) else {
        return Err(p.status);
    };
    let rule = Rule {
        phase: Phase::ListRules,
        state: St::Blank,
        back: Back::ListRules,
        item: Some(root),
        failed_state: St::Failed,
        end: EndCb::ListRulesEnd,
        block_end: T::Undef,
        ..Rule::default()
    };
    if !p.push_rule(St::Stub, rule) {
        return Err(p.status);
    }
    p.status = p.run();
    match p.status {
        Status::Ok => Ok(Sheet {
            ast: p.ast,
            rules: p.sheet,
            root,
        }),
        st => Err(st),
    }
}

/// The kind of at-rule a name creates (`lxb_css_at_rule_by_name`, which
/// compares lower-cased).
fn at_kind(name: &[u8]) -> AtKind {
    if name.eq_ignore_ascii_case(b"media") {
        AtKind::Media
    } else if name.eq_ignore_ascii_case(b"font-face") {
        AtKind::FontFace
    } else if name.eq_ignore_ascii_case(b"namespace") {
        AtKind::Namespace
    } else {
        AtKind::Custom
    }
}

impl<'a> Parser<'a> {
    /* ==== helpers ==== */

    /// `lxb_css_syntax_token(parser->tkz)` inside a phase; a failure is
    /// `lxb_css_syntax_parser_failed(parser, tkz->status)`.
    pub(super) fn raw_token(&mut self) -> Option<Token> {
        match self.tkz.token() {
            Some(t) => Some(t),
            None => {
                self.status = self.tkz.status;
                None
            }
        }
    }

    /// `lxb_css_syntax_token_parser_do_phase_again`.
    pub(super) fn phase_again(&mut self) -> Option<Token> {
        self.fake_null = true;
        None
    }

    /// `lxb_css_syntax_set_return`.
    pub(super) fn set_return(&mut self, value: Option<ItemId>) {
        if self.r >= 1 {
            self.rules[self.r - 1].returned = value;
        }
    }

    /// A new rule object. Out of memory is `lxb_css_parser_memory_fail`.
    pub(super) fn item_create(&mut self, item: Item) -> Option<ItemId> {
        let Ok(id) = ItemId::try_from(self.sheet.nodes.len()) else {
            self.memory_fail();
            return None;
        };
        if self
            .sheet
            .nodes
            .falloc_push(Node { item, next: None })
            .is_err()
        {
            self.memory_fail();
            return None;
        }
        Some(id)
    }

    /// `lxb_css_rule_list_append` and `..._declaration_list_append`.
    pub(super) fn item_append(&mut self, list: ItemId, rule: ItemId) {
        let last = match self.sheet.item_mut(list) {
            Item::List { first, last } | Item::DeclarationList { first, last } => {
                let prev = *last;
                if first.is_none() {
                    *first = Some(rule);
                }
                *last = Some(rule);
                prev
            }
            _ => return,
        };
        if let Some(prev) = last {
            self.sheet.nodes[prev as usize].next = Some(rule);
        }
    }

    /// The `u.*->block` an at-rule end stores.
    pub(super) fn at_set(&mut self, at: Option<ItemId>, to_undef: bool, block: Option<ItemId>) {
        if let Some(Item::At { kind, block: b, .. }) = at.map(|id| self.sheet.item_mut(id)) {
            if to_undef {
                *kind = AtKind::Undef;
            }
            *b = block;
        }
    }

    /* ==== phases: css/syntax/parser.c ==== */

    /// `lxb_css_syntax_parser_list_rules`.
    pub(super) fn phase_list_rules(&mut self, mut token: Token, r: usize) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        self.rules[r].skip_consume = true;
        loop {
            self.rules[r].offset = token.offset + token.length;
            match token.ty {
                T::Whitespace | T::Cdc | T::Cdo => {
                    self.tkz.consume();
                    token = self.raw_token()?;
                }
                T::AtKeyword => {
                    self.rules[r].phase = Phase::ListRulesAt;
                    return Some(token);
                }
                T::Eof => break,
                _ => {
                    if self.rules[r].block_end == token.ty && self.rules[r].deep == 0 {
                        break;
                    }
                    self.rules[r].phase = Phase::ListRulesQualified;
                    return Some(token);
                }
            }
        }
        self.rules[r].phase = Phase::End;
        Some(Self::token_end(0))
    }

    /// `lxb_css_syntax_parser_list_rules_qualified`.
    pub(super) fn phase_list_rules_qualified(&mut self, r: usize) -> Option<Token> {
        self.rules[r].skip_consume = false;
        let block_end = self.rules[r].block_end;
        self.qualified_begin(St::ListRulesNext, block_end, false)
    }

    /// `lxb_css_syntax_parser_at_begin` with `lxb_css_state_at_rule_begin`.
    pub(super) fn at_begin(
        &mut self,
        token: Token,
        r: usize,
        back: St,
        nested: bool,
    ) -> Option<Token> {
        self.rules[r].skip_consume = false;
        let name = self.dup(token.string)?;
        let kind = at_kind(self.tkz.string(token.string));
        let at = self.item_create(Item::At {
            kind,
            name,
            prelude_begin: 0,
            prelude_end: 0,
            block: None,
        })?;
        let rule = Rule {
            phase: Phase::AtRule,
            state: St::AtPrelude,
            back: Back::AtToPrelude,
            item: Some(at),
            failed_state: St::SkipToEnd,
            end: EndCb::AtRuleEnd(kind),
            block_end: self.rules[r].block_end,
            nested,
            ..Rule::default()
        };
        if !self.push_rule(back, rule) {
            return None;
        }
        self.tkz.consume();
        let token = self.raw_token()?;
        if token.ty == T::Whitespace {
            self.tkz.consume();
        }
        self.phase_again()
    }

    /// The bracket counting the prelude and value phases share: push the
    /// closer an opener needs. `None` is an allocation failure.
    pub(super) fn open_bracket(&mut self, ty: T, r: usize) -> Option<bool> {
        let closer = match ty {
            T::LsBracket => T::RsBracket,
            T::Function | T::LParenthesis => T::RParenthesis,
            T::LcBracket => T::RcBracket,
            _ => return Some(false),
        };
        if !self.types_push(closer) {
            return None;
        }
        self.rules[r].deep += 1;
        Some(true)
    }

    /// Pop the closer `ty` closes, when it is the innermost one.
    pub(super) fn close_bracket(&mut self, ty: T, r: usize) {
        if self.rules[r].deep != 0 && self.types.last() == Some(&ty) {
            self.types.pop();
            self.rules[r].deep -= 1;
        }
    }

    /// `lxb_css_syntax_parser_at_rule` and `..._qualified_rule`: the prelude,
    /// up to the block or the end of the rule.
    pub(super) fn prelude_phase(&mut self, token: Token, r: usize, at: bool) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        self.rules[r].offset = token.offset + token.length;
        let (end_phase, block_phase) = if at {
            (Phase::AtPreludeEnd, Phase::AtBlock)
        } else {
            (Phase::QualifiedPreludeEnd, Phase::QualifiedBlock)
        };
        let deep = self.rules[r].deep;
        let done = if self.rules[r].block_end == token.ty && deep == 0 {
            end_phase
        } else {
            match token.ty {
                T::LcBracket if deep == 0 => {
                    let rule = &mut self.rules[r];
                    rule.phase = block_phase;
                    rule.back = Back::ToEnd;
                    rule.skip_consume = true;
                    return Some(Self::token_end(token.offset));
                }
                T::LsBracket | T::Function | T::LParenthesis | T::LcBracket => {
                    self.open_bracket(token.ty, r)?;
                    return Some(token);
                }
                T::RcBracket => {
                    if deep != 0 {
                        self.close_bracket(token.ty, r);
                    } else if self.rules[r].nested {
                        // Lexbor logs "Unexpected token in qualified rule".
                        return self.prelude_done(token, r, end_phase);
                    }
                    return Some(token);
                }
                T::RsBracket | T::RParenthesis => {
                    self.close_bracket(token.ty, r);
                    return Some(token);
                }
                T::Semicolon if at && deep == 0 => Phase::AtPreludeEndConsumeToken,
                T::Eof => end_phase,
                _ => return Some(token),
            }
        };
        self.prelude_done(token, r, done)
    }

    pub(super) fn prelude_done(&mut self, token: Token, r: usize, phase: Phase) -> Option<Token> {
        let rule = &mut self.rules[r];
        rule.phase = phase;
        rule.skip_consume = true;
        Some(Self::token_end(token.offset))
    }

    /// `lxb_css_syntax_parser_at_rule`.
    pub(super) fn phase_at_rule(&mut self, token: Token, r: usize) -> Option<Token> {
        self.prelude_phase(token, r, true)
    }

    /// `lxb_css_syntax_parser_qualified_rule`.
    pub(super) fn phase_qualified(&mut self, token: Token, r: usize) -> Option<Token> {
        self.prelude_phase(token, r, false)
    }

    /// `lxb_css_syntax_parser_at_block`.
    pub(super) fn phase_at_block(&mut self, token: Token, r: usize) -> Option<Token> {
        let item = self.rules[r].item;
        self.at_prelude_end(item, token);
        /* `lxb_css_at_rule_namespace_block`: a namespace has no block. */
        if let Some(Item::At {
            kind: AtKind::Namespace,
            ..
        }) = item.map(|id| self.sheet.item(id))
        {
            self.rules[self.r].failed = true;
        }
        self.block_begin(r, St::AtPrelude)
    }

    /// `lxb_css_syntax_parser_at_prelude_end` and `..._consule_token`.
    pub(super) fn phase_at_prelude_end(
        &mut self,
        token: Token,
        r: usize,
        consume: bool,
    ) -> Option<Token> {
        if consume {
            self.rules[r].phase = Phase::EndConsumeToken;
        } else {
            self.rules[r].phase = Phase::End;
            self.rules[r].state = St::Success;
        }
        let item = self.rules[r].item;
        self.at_prelude_end(item, token);
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_qualified_rule_block`.
    pub(super) fn phase_qualified_block(&mut self, token: Token, r: usize) -> Option<Token> {
        let (item, failed) = (self.rules[r].item, self.rules[r].failed);
        self.status = self.qualified_prelude_end(item, token, failed);
        if self.status != Status::Ok {
            return None;
        }
        self.block_begin(r, St::QualifiedPrelude)
    }

    /// `lxb_css_syntax_parser_qualified_prelude_end`.
    pub(super) fn phase_qualified_prelude_end(&mut self, token: Token, r: usize) -> Option<Token> {
        let (item, failed) = (self.rules[r].item, self.rules[r].failed);
        self.status = self.qualified_prelude_end(item, token, failed);
        if self.status != Status::Ok {
            return None;
        }
        self.rules[r].phase = Phase::End;
        self.rules[r].state = St::Success;
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_block_begin` with the block callback every
    /// at-rule and qualified rule uses (`lxb_css_state_at_rule_block_begin`,
    /// `..._qualified_rule_block_begin`).
    pub(super) fn block_begin(&mut self, r: usize, back: St) -> Option<Token> {
        self.rules[r].skip_consume = false;
        let list = self.item_create(Item::List {
            first: None,
            last: None,
        })?;
        let rule = Rule {
            phase: Phase::Block,
            state: St::Blank,
            back: Back::Block,
            item: Some(list),
            failed_state: St::Failed,
            end: EndCb::BlockEnd,
            ..Rule::default()
        };
        if !self.push_rule(back, rule) {
            return None;
        }
        self.tkz.consume();
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_block`.
    pub(super) fn phase_block(&mut self, mut token: Token, r: usize) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        let phase = loop {
            self.rules[r].offset = token.offset + token.length;
            match token.ty {
                T::Semicolon | T::Whitespace => {
                    self.tkz.consume();
                    token = self.raw_token()?;
                }
                T::RcBracket => {
                    if self.rules[r].deep == 0 {
                        break Phase::EndConsumeToken;
                    }
                    self.close_bracket(token.ty, r);
                    return Some(token);
                }
                T::Eof => break Phase::End,
                T::AtKeyword => break Phase::BlockAt,
                T::Ident if self.tkz.lookup_colon() => break Phase::BlockDeclarations,
                _ => break Phase::BlockQualified,
            }
        };
        self.rules[r].phase = phase;
        self.rules[r].skip_consume = true;
        Some(Self::token_end(0))
    }

    /// `lxb_css_syntax_parser_block_declarations` with
    /// `lxb_css_state_declarations_begin`.
    pub(super) fn phase_block_declarations(&mut self, r: usize) -> Option<Token> {
        self.rules[r].skip_consume = false;
        let list = self.item_create(Item::DeclarationList {
            first: None,
            last: None,
        })?;
        let rule = Rule {
            phase: Phase::DeclarationsBegin,
            state: St::Blank,
            back: Back::DeclarationToValue,
            item: Some(list),
            failed_state: St::DeclarationsBad,
            end: EndCb::DeclarationsEnd,
            block_end: T::RcBracket,
            nested: true,
            ..Rule::default()
        };
        if !self.push_rule(St::BlockNext, rule) {
            return None;
        }
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_block_qualified`.
    pub(super) fn phase_block_qualified(&mut self, r: usize) -> Option<Token> {
        self.rules[r].skip_consume = false;
        self.qualified_begin(St::BlockNext, T::Semicolon, true)
    }

    /// The qualified-rule push with `lxb_css_state_qualified_rule_begin`.
    pub(super) fn qualified_begin(&mut self, back: St, stop: T, nested: bool) -> Option<Token> {
        let style = self.item_create(Item::Style {
            selector: None,
            prelude_begin: 0,
            prelude_end: 0,
            declarations: None,
            child: None,
        })?;
        let rule = Rule {
            phase: Phase::Qualified,
            state: St::QualifiedPrelude,
            back: Back::QualifiedToPrelude,
            item: Some(style),
            failed_state: St::SkipToEnd,
            end: EndCb::QualifiedEnd,
            block_end: stop,
            nested,
            ..Rule::default()
        };
        if !self.push_rule(back, rule) {
            return None;
        }
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_declarations_begin` with
    /// `lxb_css_state_declaration_name`.
    pub(super) fn phase_declarations_begin(&mut self, token: Token, r: usize) -> Option<Token> {
        let decl = self.declaration_create(token)?;
        let rule = &mut self.rules[r];
        rule.state = St::DeclarationValue;
        rule.phase = Phase::DeclarationsName;
        rule.item_old = rule.item;
        rule.item = Some(decl);
        rule.skip_consume = false;
        self.tkz.consume();
        self.phase_again()
    }

    /// `lxb_css_declaration_create` for the stylesheet: the name is the
    /// ident's value.
    pub(super) fn declaration_create(&mut self, token: Token) -> Option<ItemId> {
        let name = self.dup(token.string)?;
        self.item_create(Item::Declaration {
            name,
            value_begin: 0,
            value_end: 0,
            important: false,
        })
    }

    /// `lxb_css_syntax_parser_declarations_name`: the colon and the
    /// whitespace around it.
    pub(super) fn phase_declarations_name(&mut self, mut token: Token, r: usize) -> Option<Token> {
        if token.ty == T::Whitespace {
            self.tkz.consume();
            token = self.raw_token()?;
        }
        if token.ty != T::Colon {
            /* Lexbor returns NULL without a status here ("It can't be":
             * `lookup_colon` saw the colon before the phase was entered).
             * The port fails rather than end the parse as a success. */
            self.status = Status::ErrorUnexpectedData;
            return None;
        }
        self.tkz.consume();
        token = self.raw_token()?;
        if token.ty == T::Whitespace {
            self.tkz.consume();
            token = self.raw_token()?;
        }
        self.rules[r].begin = token.offset;
        self.offset.value_end = 0;
        self.rules[r].phase = Phase::DeclarationsValue;
        self.phase_declarations_value(token, r)
    }

    /// The `stop_ch` the lookups take: `}` inside a block.
    pub(super) fn stop_ch(&self, r: usize) -> u8 {
        if self.rules[r].block_end == T::RcBracket {
            b'}'
        } else {
            0
        }
    }

    /// The tail of an `!important`: `rule->important` is set, the `!` (or
    /// whatever token stands there) and the token after it are skipped, and
    /// whitespace after them.
    pub(super) fn skip_important(&mut self, r: usize, token: Token) -> Option<Token> {
        self.rules[r].important = true;
        self.offset.important_begin = token.offset;
        self.tkz.consume();
        let token = self.raw_token()?;
        self.offset.important_end = token.offset + token.length;
        self.tkz.consume();
        let token = self.raw_token()?;
        if token.ty == T::Whitespace {
            self.tkz.consume();
            return self.raw_token();
        }
        Some(token)
    }

    /// `lxb_css_syntax_parser_declarations_value`.
    pub(super) fn phase_declarations_value(&mut self, mut token: Token, r: usize) -> Option<Token> {
        if self.rules[r].offset > token.offset {
            return Some(token);
        }
        loop {
            self.rules[r].offset = token.offset + token.length;
            let deep = self.rules[r].deep;
            if self.rules[r].block_end == token.ty && deep == 0 {
                return self.value_done(token, r);
            }
            match token.ty {
                T::Whitespace => {
                    if deep != 0 {
                        return Some(token);
                    }
                    let stop = self.rules[r].block_end;
                    if !self.tkz.lookup_declaration_ws_end(stop, self.stop_ch(r)) {
                        return Some(token);
                    }
                    self.offset.value_end = token.offset;
                    self.tkz.consume();
                    token = self.raw_token()?;
                    if token.ty == T::Delim {
                        token = self.skip_important(r, token)?;
                    } else {
                        self.offset.important_begin = 0;
                        self.offset.important_end = 0;
                    }
                }
                T::Semicolon => {
                    if deep != 0 {
                        return Some(token);
                    }
                    self.rules[r].phase = Phase::DeclarationsNext;
                    let offset = token.offset;
                    self.offset.value_begin = self.rules[r].begin;
                    self.offset.end = offset;
                    if self.offset.value_end == 0 {
                        self.offset.value_end = offset;
                        self.offset.important_begin = 0;
                        self.offset.important_end = 0;
                    }
                    self.tkz.consume();
                    return Some(Self::token_end(offset));
                }
                T::Delim => {
                    if token.delim != u32::from(b'!') {
                        return Some(token);
                    }
                    let stop = self.rules[r].block_end;
                    if !self.tkz.lookup_important(stop, self.stop_ch(r)) {
                        return Some(token);
                    }
                    self.offset.value_end = token.offset;
                    token = self.skip_important(r, token)?;
                }
                T::LsBracket | T::Function | T::LParenthesis | T::LcBracket => {
                    self.open_bracket(token.ty, r)?;
                    return Some(token);
                }
                T::RcBracket | T::RsBracket | T::RParenthesis => {
                    self.close_bracket(token.ty, r);
                    return Some(token);
                }
                T::Eof => return self.value_done(token, r),
                _ => return Some(token),
            }
        }
    }

    /// The `done:` of `lxb_css_syntax_parser_declarations_value`.
    pub(super) fn value_done(&mut self, token: Token, r: usize) -> Option<Token> {
        self.offset.value_begin = self.rules[r].begin;
        self.offset.end = token.offset;
        if self.offset.value_end == 0 {
            self.offset.value_end = token.offset;
            self.offset.important_begin = 0;
            self.offset.important_end = 0;
        }
        self.rules[r].phase = Phase::DeclarationEnd;
        self.rules[r].skip_consume = true;
        Some(Self::token_end(token.offset))
    }

    /// `lxb_css_syntax_parser_declarations_next`.
    pub(super) fn phase_declarations_next(&mut self, mut token: Token, r: usize) -> Option<Token> {
        if self.rules[r].state != St::Success {
            self.rules[r].skip_consume = true;
            return Some(Self::token_end(token.offset));
        }
        self.declaration_end(r);
        loop {
            match token.ty {
                T::Semicolon | T::Whitespace => {
                    self.tkz.consume();
                    token = self.raw_token()?;
                }
                T::Eof => {
                    self.rules[r].phase = Phase::DeclarationsEnd;
                    break;
                }
                T::Ident if self.tkz.lookup_colon() => {
                    self.rules[r].skip_consume = true;
                    let decl = self.declaration_create(token)?;
                    let rule = &mut self.rules[r];
                    rule.state = St::DeclarationValue;
                    rule.item = Some(decl);
                    self.tkz.consume();
                    self.rules[r].phase = Phase::DeclarationsName;
                    break;
                }
                _ => {
                    /* A declaration list that is not nested drops up to the
                     * next `;` instead; a stylesheet has none. */
                    self.rules[r].phase = Phase::DeclarationsEnd;
                    break;
                }
            }
        }
        let rule = &mut self.rules[r];
        rule.skip_consume = false;
        rule.important = false;
        rule.failed = false;
        self.phase_again()
    }

    /// `lxb_css_syntax_parser_declaration_end` (`..._end_handler` with
    /// `skip_token` false).
    pub(super) fn phase_declaration_end(&mut self, token: Token, r: usize) -> Option<Token> {
        if self.rules[r].state != St::Success {
            self.rules[r].skip_consume = true;
            return Some(Self::token_end(token.offset));
        }
        self.declaration_end(r);
        self.phase_declarations_end(token, r)
    }

    /// `lxb_css_syntax_parser_declarations_end` (`..._end_h` with
    /// `skip_token` false).
    pub(super) fn phase_declarations_end(&mut self, token: Token, r: usize) -> Option<Token> {
        let rule = &mut self.rules[r];
        rule.item = rule.item_old;
        let item = rule.item;
        self.set_return(item);
        self.call_back(token)
    }

    /* ==== callbacks: css/state.c, css/at_rule/state.c ==== */

    /// `lxb_css_state_declaration_end`: the declaration takes the offsets
    /// and joins its list. A failed one becomes `_undef` in Lexbor, which
    /// keeps the same offsets.
    pub(super) fn declaration_end(&mut self, r: usize) {
        let rule = self.rules[r];
        let (Some(list), Some(decl)) = (rule.item_old, rule.item) else {
            return;
        };
        if let Item::Declaration {
            value_begin,
            value_end,
            important,
            ..
        } = self.sheet.item_mut(decl)
        {
            *value_begin = self.offset.value_begin;
            *value_end = self.offset.value_end;
            *important = rule.important;
        }
        self.item_append(list, decl);
    }

    /// `lxb_css_state_list_rules_next`.
    pub(super) fn list_rules_next(&mut self) -> bool {
        let rule = self.rules[self.r];
        if let (Some(list), Some(returned)) = (rule.item, rule.returned) {
            self.item_append(list, returned);
        }
        self.success()
    }

    /// `lxb_css_state_block_next`.
    pub(super) fn block_next(&mut self) -> bool {
        let rule = self.rules[self.r];
        if let (Some(list), Some(returned)) = (rule.item, rule.returned) {
            self.item_append(list, returned);
        }
        self.success()
    }

    /// The loop every prelude-skipping state runs: consume to the end.
    pub(super) fn skip_tokens(&mut self, mut token: Token) {
        while token.ty != T::End {
            self.consume();
            match self.token() {
                Some(t) => token = t,
                None => break,
            }
        }
    }

    /// `lxb_css_at_rule_*_prelude_failed` and
    /// `lxb_css_state_qualified_rule_prelude_failed`.
    pub(super) fn skip_to_end(&mut self, token: Token) -> bool {
        self.skip_tokens(token);
        self.success()
    }

    /// `lxb_css_at_rule_*_prelude`.
    pub(super) fn at_rule_prelude(&mut self, token: Token) -> bool {
        let item = self.rules[self.r].item;
        let mut font_face = false;
        if let Some(Item::At {
            kind,
            prelude_begin,
            ..
        }) = item.map(|id| self.sheet.item_mut(id))
        {
            *prelude_begin = token.offset;
            font_face = *kind == AtKind::FontFace;
        }
        if font_face {
            if token.ty != T::End {
                return self.failed();
            }
            return self.success();
        }
        self.skip_tokens(token);
        self.success()
    }

    /// `lxb_css_at_rule_*_prelude_end`: every kind records where the
    /// prelude ends (`_undef` and `_custom` also copy it, which the reader
    /// takes from the offsets instead).
    pub(super) fn at_prelude_end(&mut self, item: Option<ItemId>, token: Token) {
        if let Some(Item::At { prelude_end, .. }) = item.map(|id| self.sheet.item_mut(id)) {
            *prelude_end = token.offset;
        }
    }

    /// `lxb_css_at_rule_*_end`, by the kind the rule was created as.
    pub(super) fn at_rule_end(
        &mut self,
        kind: AtKind,
        at: Option<ItemId>,
        failed: bool,
    ) -> Option<Status> {
        let block = self.rules[self.r].returned;
        let to_undef = match kind {
            AtKind::Undef | AtKind::Custom => false,
            AtKind::Namespace | AtKind::FontFace => failed,
            AtKind::Media => block.is_none() || failed,
        };
        self.at_set(at, to_undef, block);
        self.set_return(at);
        Some(Status::Ok)
    }

    /// `lxb_css_state_qualified_rule_prelude`.
    pub(super) fn qualified_rule_prelude(&mut self, token: Token) -> bool {
        /* lxb_css_selectors_clean */
        self.sel = Selectors {
            list: None,
            list_last: None,
            parent: None,
            combinator: comb::DESCENDANT,
            comb_default: comb::DESCENDANT,
            err_in_function: false,
        };
        let r = self.r;
        self.rules[r].state = St::ComplexList;
        if let Some(Item::Style { prelude_begin, .. }) =
            self.rules[r].item.map(|id| self.sheet.item_mut(id))
        {
            *prelude_begin = token.offset;
        }
        false
    }

    /// `lxb_css_state_qualified_rule_prelude_end`: the selectors, or - when
    /// they failed - a bad style rule in the style rule's place.
    pub(super) fn qualified_prelude_end(
        &mut self,
        item: Option<ItemId>,
        token: Token,
        failed: bool,
    ) -> Status {
        let Some(id) = item else {
            return Status::Ok;
        };
        let list = self.sel.list;
        let it = self.sheet.item_mut(id);
        if let Item::Style {
            selector,
            prelude_begin,
            prelude_end,
            ..
        } = it
        {
            if !failed {
                *selector = list;
                *prelude_end = token.offset;
            } else {
                *it = Item::BadStyle {
                    prelude_begin: *prelude_begin,
                    prelude_end: token.offset,
                    declarations: None,
                    child: None,
                };
            }
        }
        Status::Ok
    }

    /// `lxb_css_state_qualified_rule_end`: the block's first declaration
    /// list becomes the rule's declarations, the rest its child rules.
    pub(super) fn qualified_rule_end(
        &mut self,
        item: Option<ItemId>,
        _failed: bool,
    ) -> Option<Status> {
        let list = self.rules[self.r].returned;
        let mut declrs = None;
        if let Some(l) = list {
            let first = match self.sheet.item(l) {
                Item::List { first, .. } => *first,
                _ => None,
            };
            if let Some(f) = first {
                if matches!(self.sheet.item(f), Item::DeclarationList { .. }) {
                    declrs = Some(f);
                    let next = self.sheet.nodes[f as usize].next;
                    self.sheet.nodes[f as usize].next = None;
                    if let Item::List { first, last } = self.sheet.item_mut(l) {
                        if *last == Some(f) {
                            *last = None;
                        }
                        *first = next;
                    }
                }
            }
        }
        if let Some(id) = item {
            match self.sheet.item_mut(id) {
                Item::Style {
                    declarations,
                    child,
                    ..
                }
                | Item::BadStyle {
                    declarations,
                    child,
                    ..
                } => {
                    *declarations = declrs;
                    *child = list;
                }
                _ => {}
            }
        }
        self.set_return(item);
        Some(Status::Ok)
    }

    /// The value state (see [`St::DeclarationValue`]).
    pub(super) fn declaration_value(&mut self, token: Token) -> bool {
        self.skip_tokens(token);
        self.success()
    }

    /// `lxb_css_state_declarations_bad`.
    pub(super) fn declarations_bad(&mut self, token: Token) -> bool {
        if self.rules[self.r].item.is_none() {
            let Some(decl) = self.item_create(Item::Declaration {
                name: Span::default(),
                value_begin: 0,
                value_end: 0,
                important: false,
            }) else {
                return true;
            };
            self.rules[self.r].item = Some(decl);
        }
        self.skip_tokens(token);
        self.success()
    }
}
