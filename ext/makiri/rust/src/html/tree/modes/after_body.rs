//! The "after body" insertion mode (`insertion_mode/after_body.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_insertion_mode_after_body`: Lexbor switches on the tag
/// alone, so an end tag falls where its start tag would, `html` aside.
pub fn after_body(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            let html = tree.open_first();
            let _ = tree.insert_comment(tkz, tok, html).or_abort(tree);
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            let html = tree.open_first();
            let _ = tree.insert_processing_instruction(tkz, html).or_abort(tree);
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoafbomo),
        res::TAG_HTML => {
            if tok.is_close() {
                if tree.fragment.is_some() {
                    tree.parse_error(tok, E::Unclto);
                    return true;
                }
                tree.mode = Mode::AfterAfterBody;
                return true;
            }
            return super::in_body::in_body(tree, tkz, tok);
        }
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
