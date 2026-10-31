//! The "in table body" insertion mode (`insertion_mode/in_table_body.c`).

use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_clear_stack_back_to_table_body`.
fn clear_stack_back_to_table_body(tree: &mut TreeBuilder<'_>) {
    tree.clear_stack_back_to(&[
        res::TAG_TBODY,
        res::TAG_TFOOT,
        res::TAG_THEAD,
        res::TAG_TEMPLATE,
        res::TAG_HTML,
    ]);
}

/// `..._in_table_body_ct_open_closed`.
fn ct_open_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree.element_in_scope_tbody_thead_tfoot().is_none() {
        tree.parse_error(tok, E::Mielinsc);
        return true;
    }
    clear_stack_back_to_table_body(tree);
    tree.open_pop();
    tree.mode = Mode::InTable;
    false
}

/// `lxb_html_tree_insertion_mode_in_table_body`.
pub fn in_table_body(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD => {
                if tree
                    .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE_TABLE)
                    .is_none()
                {
                    tree.parse_error(tok, E::Mielinsc);
                    return true;
                }
                clear_stack_back_to_table_body(tree);
                tree.open_pop();
                tree.mode = Mode::InTable;
                true
            }
            res::TAG_TABLE => ct_open_closed(tree, tok),
            res::TAG_BODY
            | res::TAG_CAPTION
            | res::TAG_COL
            | res::TAG_COLGROUP
            | res::TAG_HTML
            | res::TAG_TD
            | res::TAG_TH
            | res::TAG_TR => {
                tree.parse_error(tok, E::Unclto);
                true
            }
            _ => super::in_table::in_table(tree, tkz, tok),
        };
    }
    match tok.tag_id {
        res::TAG_TR => {
            clear_stack_back_to_table_body(tree);
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.mode = Mode::InRow;
            true
        }
        res::TAG_TH | res::TAG_TD => {
            tree.parse_error(tok, E::Unto);
            clear_stack_back_to_table_body(tree);
            let mut fake = *tok;
            fake.tag_id = res::TAG_TR;
            fake.has_attrs = false;
            /* The implied `<tr>` is the parser's, not the `<td>`'s: no
             * source position (`created_offset`). */
            fake.begin = None;
            if tree
                .insert_html_element(tkz, &fake)
                .or_abort(tree)
                .is_none()
            {
                return true;
            }
            tree.mode = Mode::InRow;
            false
        }
        res::TAG_CAPTION
        | res::TAG_COL
        | res::TAG_COLGROUP
        | res::TAG_TBODY
        | res::TAG_TFOOT
        | res::TAG_THEAD => ct_open_closed(tree, tok),
        _ => super::in_table::in_table(tree, tkz, tok),
    }
}
