//! The "after after frameset" insertion mode
//! (`insertion_mode/after_after_frameset.c`).

use crate::compat::{self, Fix};
use crate::html::dom::NodeId;
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{is_ws, OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_insertion_mode_after_after_frameset`.
pub fn after_after_frameset(
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
        res::TAG_NOFRAMES => return super::in_head::in_head(tree, tkz, tok),
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                let mut ws = *tok;
                ws.skip_ws_begin(tkz);
                if ws.text_start == ws.text_end {
                    return super::in_body::in_body(tree, tkz, tok);
                }
                /* The Standard takes the token character by character: its
                 * whitespace goes to "in body" (reconstruct the active
                 * formatting elements, insert) and the rest is ignored.
                 * Lexbor ignores the whole token (P7). */
                if compat::fixed(Fix::P7) && tok.text(tkz).iter().any(|&c| is_ws(c)) {
                    if tree.afe_reconstruct(tkz).or_abort(tree).is_none() {
                        return true;
                    }
                    tree.parse_error(tok, E::Unto);
                    /* Consumed either way: an aborted parse is done with it. */
                    let _ = super::in_frameset::whitespace_only(tree, tkz, tok);
                    return true;
                }
            }
            tree.parse_error(tok, E::Unto);
        }
    }
    true
}
