//! The "in frameset" insertion mode (`insertion_mode/in_frameset.c`).

use crate::falloc::try_vec_with_capacity;
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{is_ws, OrAbort, Tok, TreeBuilder, TreeStatus};

/// The text case shared with "after frameset": insert the token's
/// whitespace (`lexbor_str_stay_only_whitespace`), and say whether that was
/// all of it. `None` when the parse was aborted.
pub(super) fn whitespace_only(
    tree: &mut TreeBuilder<'_>,
    tkz: &Tokenizer<'_, '_>,
    tok: &Tok,
) -> Option<bool> {
    let data = tok.text(tkz);
    let Some(mut ws) = try_vec_with_capacity::<u8>(data.len()) else {
        tree.abort_with(TreeStatus::Oom);
        return None;
    };
    ws.extend(data.iter().copied().filter(|&c| is_ws(c)));
    if !ws.is_empty() {
        tree.insert_character_for_data(&ws).or_abort(tree)?;
    }
    Some(ws.len() == data.len())
}

/// `lxb_html_tree_insertion_mode_in_frameset`.
pub fn in_frameset(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        if tok.tag_id == res::TAG_FRAMESET {
            if tree.current_node() == tree.open_first() {
                tree.parse_error(tok, E::Unelinopelst);
                return true;
            }
            tree.open_pop();
            let is = tree.current_is(res::TAG_FRAMESET);
            if tree.fragment.is_none() && !is {
                tree.mode = crate::html::tree::Mode::AfterFrameset;
            }
            return true;
        }
        tree.parse_error(tok, E::Unto);
        return true;
    }
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            let _ = tree.insert_comment(tkz, tok, None).or_abort(tree);
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            let _ = tree.insert_processing_instruction(tkz, None).or_abort(tree);
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoinfrmo),
        res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG_FRAMESET => {
            let _ = tree.insert_html_element(tkz, tok).or_abort(tree);
        }
        res::TAG_FRAME => {
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.open_pop();
            tree.acknowledge_token_self_closing(tok);
        }
        res::TAG_NOFRAMES => return super::in_head::in_head(tree, tkz, tok),
        res::TAG__END_OF_FILE => {
            if tree.current_node() != tree.open_first() {
                tree.parse_error(tok, E::Unelinopelst);
            }
        }
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                match whitespace_only(tree, tkz, tok) {
                    None | Some(true) => return true,
                    Some(false) => {}
                }
            }
            tree.parse_error(tok, E::Unto);
        }
    }
    true
}
