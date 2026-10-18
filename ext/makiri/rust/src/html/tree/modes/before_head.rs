//! The "before head" insertion mode (`insertion_mode/before_head.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_before_head`.
pub fn before_head(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return closed(tree, tkz, tok);
    }
    open(tree, tkz, tok)
}

fn open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).is_err() {
                return tree.abort_with(TreeStatus::Oom);
            }
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree.insert_processing_instruction(tkz, tok, None).is_err() {
                return tree.process_abort();
            }
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoinbehemo),
        res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG_HEAD => {
            if let Err(st) = head(tree, tkz, tok) {
                return tree.abort_with(st);
            }
            tree.mode = Mode::InHead;
        }
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                tok.skip_ws_begin(tkz);
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            return anything_else(tree, tkz);
        }
    }
    true
}

fn closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    match tok.tag_id {
        res::TAG_HEAD | res::TAG_BODY | res::TAG_HTML | res::TAG_BR => anything_else(tree, tkz),
        _ => {
            tree.parse_error(tok, E::Uncltoinbehemo);
            true
        }
    }
}

/// `lxb_html_tree_insertion_mode_before_head_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>) -> bool {
    let fake = Tok::fake(res::TAG_HEAD);
    if let Err(st) = head(tree, tkz, &fake) {
        return tree.abort_with(st);
    }
    tree.mode = Mode::InHead;
    false
}

/// `lxb_html_tree_insertion_mode_before_head_head`.
fn head(tree: &mut TreeBuilder<'_>, tkz: &Tokenizer<'_, '_>, tok: &Tok) -> Result<(), TreeStatus> {
    let el = tree.insert_html_element(tkz, tok)?;
    tree.head = Some(el);
    Ok(())
}
