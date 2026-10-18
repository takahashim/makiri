//! The "after frameset" insertion mode (`insertion_mode/after_frameset.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder};

/// `lxb_html_tree_insertion_mode_after_frameset`.
pub fn after_frameset(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).is_err() {
                return tree.process_abort();
            }
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree.insert_processing_instruction(tkz, tok, None).is_err() {
                return tree.process_abort();
            }
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoaffrmo),
        res::TAG_HTML => {
            if tok.is_close() {
                tree.mode = Mode::AfterAfterFrameset;
                return true;
            }
            return super::in_body::in_body(tree, tkz, tok);
        }
        res::TAG_NOFRAMES => return super::in_head::in_head(tree, tkz, tok),
        res::TAG__END_OF_FILE => {}
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                match super::in_frameset::whitespace_only(tree, tkz, tok) {
                    None | Some(true) => return true,
                    Some(false) => {}
                }
            }
            tree.parse_error(tok, E::Unto);
        }
    }
    true
}
