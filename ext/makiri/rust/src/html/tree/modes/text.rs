//! The "text" insertion mode (`insertion_mode/text.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{OrAbort, Tok, TreeBuilder};

/// `lxb_html_tree_insertion_mode_text`.
pub fn text(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__TEXT => {
            if tree.insert_character(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
        }
        res::TAG__END_OF_FILE => {
            tree.parse_error(tok, E::Unenoffi);
            tree.open_pop();
            tree.mode = tree.original_mode;
            return false;
        }
        _ => {
            /* Any end tag, `script` and `style` included. */
            tree.open_pop();
            tree.mode = tree.original_mode;
        }
    }
    true
}
