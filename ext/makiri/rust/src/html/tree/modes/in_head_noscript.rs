//! The "in head noscript" insertion mode
//! (`insertion_mode/in_head_noscript.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_in_head_noscript`.
pub fn in_head_noscript(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tok.is_close() {
        return closed(tree, tok);
    }
    open(tree, tkz, tok)
}

fn open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoinhenomo),
        res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG__EM_COMMENT
        | res::TAG__PROCESSINGINSTRUCTION
        | res::TAG_BASEFONT
        | res::TAG_BGSOUND
        | res::TAG_LINK
        | res::TAG_META
        | res::TAG_NOFRAMES
        | res::TAG_STYLE => return super::in_head::in_head(tree, tkz, tok),
        res::TAG_HEAD | res::TAG_NOSCRIPT => tree.parse_error(tok, E::Unto),
        _ => {
            /* Copied from "in head". */
            if tok.tag_id == res::TAG__TEXT {
                let ws = tok.split_ws_begin(tkz);
                if ws.text_start != ws.text_end && tree.insert_character(tkz, &ws).is_err() {
                    return tree.abort_with(TreeStatus::Oom);
                }
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            return anything_else(tree, tok);
        }
    }
    true
}

fn closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tok.tag_id == res::TAG_BR {
        return anything_else(tree, tok);
    }
    if tok.tag_id == res::TAG_NOSCRIPT {
        tree.open_pop();
        tree.mode = Mode::InHead;
        return true;
    }
    tree.parse_error(tok, E::Unto);
    true
}

/// `lxb_html_tree_insertion_mode_in_head_noscript_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    tree.open_pop();
    tree.mode = Mode::InHead;
    false
}
