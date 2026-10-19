//! The "after head" insertion mode (`insertion_mode/after_head.c`).

use crate::compat::{self, Fix};
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_after_head`.
pub fn after_head(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return closed(tree, tkz, tok);
    }
    open(tree, tkz, tok)
}

fn open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).or_abort(tree).is_none() {
                return true;
            }
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree
                .insert_processing_instruction(tkz, None)
                .or_abort(tree)
                .is_none()
            {
                return true;
            }
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoafhemo),
        res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG_BODY => {
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.frameset_ok = false;
            tree.mode = Mode::InBody;
        }
        res::TAG_FRAMESET => {
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.mode = Mode::InFrameset;
        }
        res::TAG_BASE
        | res::TAG_BASEFONT
        | res::TAG_BGSOUND
        | res::TAG_LINK
        | res::TAG_META
        | res::TAG_NOFRAMES
        | res::TAG_SCRIPT
        | res::TAG_STYLE
        | res::TAG_TEMPLATE
        | res::TAG_TITLE => {
            let Some(head) = tree.head else {
                return tree.abort_with(TreeStatus::Error);
            };
            tree.parse_error(tok, E::Unto);
            if tree.open_push(head).or_abort(tree).is_none() {
                return true;
            }
            /* Lexbor ignores whether "in head" consumed the token. */
            super::in_head::in_head(tree, tkz, tok);
            if tree.status != TreeStatus::Ok {
                return tree.process_abort();
            }
            tree.open_remove_by_node(head);
        }
        res::TAG_HEAD => tree.parse_error(tok, E::Hetoafhemo),
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                let ws = tok.split_ws_begin(tkz);
                if ws.text_start != ws.text_end
                    && tree.insert_character(tkz, &ws).or_abort(tree).is_none()
                {
                    return true;
                }
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            return anything_else(tree, tkz);
        }
    }
    true
}

fn closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG_TEMPLATE => super::in_head::in_head(tree, tkz, tok),
        res::TAG_BODY | res::TAG_HTML | res::TAG_BR => anything_else(tree, tkz),
        _ => {
            tree.parse_error(tok, E::Unclto);
            true
        }
    }
}

/// `lxb_html_tree_insertion_mode_after_head_anything_else`, with the step
/// whatwg/html#12908 added and Lexbor lacks: the implicit `<body>` sets
/// frameset-ok back to "ok", which a `<template>` in the head had set to "not
/// ok" (docs/parser_differences.ja.md P1).
fn anything_else(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>) -> bool {
    let fake = Tok::fake(res::TAG_BODY);
    if tree
        .insert_html_element(tkz, &fake)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    if compat::fixed(Fix::P1) {
        tree.frameset_ok = true;
    }
    tree.mode = Mode::InBody;
    false
}
