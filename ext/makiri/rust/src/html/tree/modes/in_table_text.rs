//! The "in table text" insertion mode (`insertion_mode/in_table_text.c`).

use crate::falloc::{Reserve, VecGrow};
use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{is_ws, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_in_table_text`.
pub fn in_table_text(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tok.tag_id == res::TAG__TEXT {
        if tok.null_count != 0 {
            tree.parse_error(tok, E::Nuch);
        }
        let data = tok.text(tkz);
        let p = &mut tree.pending;
        let start = p.bytes.len();
        if p.bytes.falloc_reserve(data.len()).is_err() {
            return tree.abort_with(TreeStatus::Oom);
        }
        let copied = if tok.null_count != 0 {
            /* Lexbor reports it a second time here. */
            tree.parse_error(tok, E::Nuch);
            tree.pending
                .bytes
                .falloc_extend(data.iter().copied().filter(|&c| c != 0))
        } else {
            tree.pending.bytes.falloc_extend_from_slice(data)
        };
        if copied.is_err() {
            return tree.abort_with(TreeStatus::Oom);
        }
        let p = &mut tree.pending;
        let end = p.bytes.len();
        if end == start {
            return true;
        }
        if p.list.falloc_push((start, end)).is_err() {
            return tree.abort_with(TreeStatus::Oom);
        }
        if p.bytes[start..end].iter().any(|&c| !is_ws(c)) {
            p.have_non_ws = true;
        }
        return true;
    }

    /* The pending text leaves the tree builder while it is inserted. */
    let pending = core::mem::take(&mut tree.pending);
    let mut failed = None;
    if pending.have_non_ws {
        tree.parse_error(tok, E::Chintate);
        tree.foster_parenting = true;
        for &(s, e) in &pending.list {
            if let Err(st) = super::in_body::text_append(tree, tkz, &pending.bytes[s..e]) {
                failed = Some(st);
                break;
            }
        }
        if failed.is_none() {
            tree.foster_parenting = false;
        }
    } else {
        for &(s, e) in &pending.list {
            if let Err(st) = tree.insert_character_for_data(&pending.bytes[s..e]) {
                failed = Some(st);
                break;
            }
        }
    }
    tree.pending = pending;
    if let Some(st) = failed {
        return tree.abort_with(st);
    }
    tree.mode = tree.original_mode;
    false
}
