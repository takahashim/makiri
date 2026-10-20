//! The "in caption" insertion mode (`insertion_mode/in_caption.c`).

use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder};

/// Close the caption: `..._caption_closed` (`consumed` true) and
/// `..._ct_open_closed` (false).
fn close_caption(tree: &mut TreeBuilder<'_>, tok: &Tok, consumed: bool) -> bool {
    if tree
        .element_in_scope(res::TAG_CAPTION, res::NS_HTML, rt::CAT_SCOPE_TABLE)
        .is_none()
    {
        tree.parse_error(tok, E::Mielinsc);
        return true;
    }
    tree.close_element(tok, res::TAG_CAPTION, None, E::Unelinopelst);
    tree.afe_up_to_last_marker();
    tree.mode = Mode::InTable;
    consumed
}

/// `lxb_html_tree_insertion_mode_in_caption`.
pub fn in_caption(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_CAPTION => close_caption(tree, tok, true),
            res::TAG_TABLE => close_caption(tree, tok, false),
            res::TAG_BODY
            | res::TAG_COL
            | res::TAG_COLGROUP
            | res::TAG_HTML
            | res::TAG_TBODY
            | res::TAG_TD
            | res::TAG_TFOOT
            | res::TAG_TH
            | res::TAG_THEAD
            | res::TAG_TR => {
                tree.parse_error(tok, E::Unclto);
                true
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
        | res::TAG_TR => close_caption(tree, tok, false),
        _ => super::in_body::in_body(tree, tkz, tok),
    }
}
