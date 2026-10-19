//! The "before html" insertion mode (`insertion_mode/before_html.c`).

use crate::html::dom::NodeId;
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_before_html`.
pub fn before_html(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return closed(tree, tok);
    }
    open(tree, tkz, tok)
}

fn open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoinbehtmo),
        res::TAG__EM_COMMENT => {
            if tree
                .insert_comment(tkz, tok, Some(NodeId::DOCUMENT))
                .or_abort(tree)
                .is_none()
            {
                return true;
            }
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree
                .insert_processing_instruction(tkz, Some(NodeId::DOCUMENT))
                .or_abort(tree)
                .is_none()
            {
                return true;
            }
        }
        res::TAG_HTML => {
            let Some(html) = tree
                .create_element_for_token(tkz, tok, res::NS_HTML, None)
                .or_abort(tree)
            else {
                return true;
            };
            if html_html(tree, html).or_abort(tree).is_none() {
                return true;
            }
            tree.mode = Mode::BeforeHead;
        }
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                tok.skip_ws_begin(tkz);
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            return anything_else(tree);
        }
    }
    true
}

fn closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    match tok.tag_id {
        res::TAG_HEAD | res::TAG_BODY | res::TAG_HTML | res::TAG_BR => anything_else(tree),
        _ => {
            tree.parse_error(tok, E::Uncltoinbehtmo);
            true
        }
    }
}

/// `lxb_html_tree_insertion_mode_before_html_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>) -> bool {
    let Some(html) = tree
        .create_element_node(res::TAG_HTML, res::NS_HTML)
        .or_abort(tree)
    else {
        return true;
    };
    if html_html(tree, html).or_abort(tree).is_none() {
        return true;
    }
    tree.mode = Mode::BeforeHead;
    false
}

/// `lxb_html_tree_insertion_mode_before_html_html`.
fn html_html(tree: &mut TreeBuilder<'_>, html: NodeId) -> Result<(), TreeStatus> {
    tree.open_push(html)?;
    tree.doc.append_child(NodeId::DOCUMENT, html);
    Ok(())
}
