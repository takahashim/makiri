//! The "in column group" insertion mode
//! (`insertion_mode/in_column_group.c`).

use crate::compat::{self, Fix};
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_in_column_group_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    let is = tree
        .current_node()
        .is_some_and(|c| tree.node_is(c, res::TAG_COLGROUP));
    if !is {
        tree.parse_error(tok, E::Mielinopelst);
        return true;
    }
    tree.open_pop();
    tree.mode = Mode::InTable;
    false
}

/// `lxb_html_tree_insertion_mode_in_column_group`.
pub fn in_column_group(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_COLGROUP => {
                let is = tree
                    .current_node()
                    .is_some_and(|c| tree.node_is(c, res::TAG_COLGROUP));
                if !is {
                    tree.parse_error(tok, E::Mielinopelst);
                    return true;
                }
                tree.open_pop();
                tree.mode = Mode::InTable;
                true
            }
            res::TAG_COL => {
                tree.parse_error(tok, E::Unclto);
                true
            }
            res::TAG_TEMPLATE => super::in_head::in_head(tree, tkz, tok),
            _ => anything_else(tree, tok),
        };
    }
    match tok.tag_id {
        res::TAG__TEXT => {
            let ws = tok.split_ws_begin(tkz);
            if ws.text_start != ws.text_end && tree.insert_character(tkz, &ws).is_err() {
                return tree.abort_with(TreeStatus::Oom);
            }
            if tok.text_start == tok.text_end {
                return true;
            }
            /* The Standard takes a text token character by character: with no
             * colgroup to pop, each non-whitespace character is ignored and
             * each whitespace one still inserted. Lexbor ignores the rest of
             * the token (P6). */
            let colgroup = tree
                .current_node()
                .is_some_and(|c| tree.node_is(c, res::TAG_COLGROUP));
            if !colgroup && compat::fixed(Fix::P6) {
                tree.parse_error(tok, E::Mielinopelst);
                return super::in_frameset::whitespace_only(tree, tkz, tok).is_some();
            }
            anything_else(tree, tok)
        }
        /* "A DOCTYPE token: parse error, ignore the token." Lexbor has no
         * branch for it, and closes the colgroup (P5). */
        res::TAG__EM_DOCTYPE if compat::fixed(Fix::P5) => {
            tree.parse_error(tok, E::Unto);
            true
        }
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).is_err() {
                return tree.abort_with(TreeStatus::Oom);
            }
            true
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree.insert_processing_instruction(tkz, None).is_err() {
                return tree.process_abort();
            }
            true
        }
        res::TAG_HTML => super::in_body::in_body(tree, tkz, tok),
        res::TAG_COL => {
            if tree.insert_html_element(tkz, tok).is_err() {
                return tree.abort_with(TreeStatus::Oom);
            }
            tree.open_pop();
            tree.acknowledge_token_self_closing(tok);
            true
        }
        res::TAG_TEMPLATE => super::in_head::in_head(tree, tkz, tok),
        res::TAG__END_OF_FILE => super::in_body::in_body(tree, tkz, tok),
        _ => anything_else(tree, tok),
    }
}
