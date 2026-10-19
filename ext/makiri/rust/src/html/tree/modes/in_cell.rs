//! The "in cell" insertion mode (`insertion_mode/in_cell.c`).

use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder};

/// `lxb_html_tree_close_cell`.
fn close_cell(tree: &mut TreeBuilder<'_>, tok: &Tok) {
    tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
    let is = tree
        .current_node()
        .is_some_and(|c| tree.node_is(c, res::TAG_TD) || tree.node_is(c, res::TAG_TH));
    if !is {
        tree.parse_error(tok, E::Mielinopelst);
    }
    tree.open_pop_until_td_th();
    tree.afe_up_to_last_marker();
    tree.mode = Mode::InRow;
}

/// `lxb_html_tree_insertion_mode_in_cell`.
pub fn in_cell(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_TD | res::TAG_TH => {
                if tree
                    .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE_TABLE)
                    .is_none()
                {
                    tree.parse_error(tok, E::Unclto);
                    return true;
                }
                tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
                let is = tree
                    .current_node()
                    .is_some_and(|c| tree.node_is(c, tok.tag_id));
                if !is {
                    tree.parse_error(tok, E::Mielinopelst);
                }
                tree.open_pop_until_tag_id(tok.tag_id, res::NS_HTML, true);
                tree.afe_up_to_last_marker();
                tree.mode = Mode::InRow;
                true
            }
            res::TAG_BODY | res::TAG_CAPTION | res::TAG_COL | res::TAG_COLGROUP | res::TAG_HTML => {
                tree.parse_error(tok, E::Unclto);
                true
            }
            res::TAG_TABLE | res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD | res::TAG_TR => {
                if tree
                    .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE_TABLE)
                    .is_none()
                {
                    tree.parse_error(tok, E::Unclto);
                    return true;
                }
                close_cell(tree, tok);
                false
            }
            _ => super::in_body::in_body(tree, tkz, tok),
        };
    }
    match tok.tag_id {
        res::TAG_CAPTION
        | res::TAG_COL
        | res::TAG_COLGROUP
        | res::TAG_TBODY
        | res::TAG_TD
        | res::TAG_TFOOT
        | res::TAG_TH
        | res::TAG_THEAD
        | res::TAG_TR => {
            if tree.element_in_scope_td_th().is_none() {
                tree.parse_error(tok, E::Mielinsc);
                return true;
            }
            close_cell(tree, tok);
            false
        }
        _ => super::in_body::in_body(tree, tkz, tok),
    }
}
