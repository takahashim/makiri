//! The "in row" insertion mode (`insertion_mode/in_row.c`).

use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_clear_stack_back_to_table_row`.
fn clear_stack_back_to_table_row(tree: &mut TreeBuilder<'_>) {
    tree.clear_stack_back_to(&[res::TAG_TR, res::TAG_TEMPLATE, res::TAG_HTML]);
}

/// Close the row (`..._tr_closed` with `consumed`, `..._ct_open_closed`
/// without), reporting `missing` when there is no row in table scope.
fn close_row(tree: &mut TreeBuilder<'_>, tok: &Tok, missing: E, consumed: bool) -> bool {
    if tree
        .element_in_scope(res::TAG_TR, res::NS_HTML, rt::CAT_SCOPE_TABLE)
        .is_none()
    {
        tree.parse_error(tok, missing);
        return true;
    }
    clear_stack_back_to_table_row(tree);
    tree.open_pop();
    tree.mode = Mode::InTableBody;
    consumed
}

/// `lxb_html_tree_insertion_mode_in_row`.
pub fn in_row(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_TR => close_row(tree, tok, E::Unclto, true),
            res::TAG_TABLE => close_row(tree, tok, E::Unto, false),
            res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD => {
                if tree
                    .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE_TABLE)
                    .is_none()
                {
                    tree.parse_error(tok, E::Unclto);
                    return true;
                }
                if tree
                    .element_in_scope(res::TAG_TR, res::NS_HTML, rt::CAT_SCOPE_TABLE)
                    .is_none()
                {
                    return true;
                }
                clear_stack_back_to_table_row(tree);
                tree.open_pop();
                tree.mode = Mode::InTableBody;
                false
            }
            res::TAG_BODY
            | res::TAG_CAPTION
            | res::TAG_COL
            | res::TAG_COLGROUP
            | res::TAG_HTML
            | res::TAG_TD
            | res::TAG_TH => {
                tree.parse_error(tok, E::Unclto);
                true
            }
            _ => super::in_table::in_table(tree, tkz, tok),
        };
    }
    match tok.tag_id {
        res::TAG_TH | res::TAG_TD => {
            clear_stack_back_to_table_row(tree);
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.mode = Mode::InCell;
            if tree.afe_push_marker().or_abort(tree).is_none() {
                return true;
            }
            true
        }
        res::TAG_CAPTION
        | res::TAG_COL
        | res::TAG_COLGROUP
        | res::TAG_TBODY
        | res::TAG_TFOOT
        | res::TAG_THEAD
        | res::TAG_TR => close_row(tree, tok, E::Unto, false),
        _ => super::in_table::in_table(tree, tkz, tok),
    }
}
