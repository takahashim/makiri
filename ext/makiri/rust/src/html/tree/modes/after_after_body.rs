//! The "after after body" insertion mode
//! (`insertion_mode/after_after_body.c`).

use crate::html::dom::NodeId;
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_insertion_mode_after_after_body`.
pub fn after_after_body(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            let _ = tree
                .insert_comment(tkz, tok, Some(NodeId::DOCUMENT))
                .or_abort(tree);
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            let _ = tree
                .insert_processing_instruction(tkz, Some(NodeId::DOCUMENT))
                .or_abort(tree);
        }
        res::TAG__EM_DOCTYPE | res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG__END_OF_FILE => {}
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                let mut ws = *tok;
                ws.skip_ws_begin(tkz);
                if ws.text_start == ws.text_end {
                    return super::in_body::in_body(tree, tkz, tok);
                }
            }
            tree.parse_error(tok, E::Unto);
            tree.mode = Mode::InBody;
            return false;
        }
    }
    true
}
