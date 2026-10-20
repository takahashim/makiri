//! The "in template" insertion mode (`insertion_mode/in_template.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// Replace the current template insertion mode with `mode`, switch to it,
/// and reprocess.
fn switch_to(tree: &mut TreeBuilder<'_>, mode: Mode) -> bool {
    tree.template_modes_pop();
    if tree.template_modes_push(mode).or_abort(tree).is_none() {
        return true;
    }
    tree.mode = mode;
    false
}

/// `lxb_html_tree_insertion_mode_in_template_end_of_file`.
fn end_of_file(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree.open_find(res::TAG_TEMPLATE, res::NS_HTML).is_none() {
        /* `lxb_html_tree_stop_parsing` */
        return true;
    }
    tree.parse_error(tok, E::Unenoffi);
    tree.open_pop_until_tag_id(res::TAG_TEMPLATE, res::NS_HTML, true);
    tree.afe_up_to_last_marker();
    tree.template_modes_pop();
    tree.reset_insertion_mode_appropriately();
    false
}

/// `lxb_html_tree_insertion_mode_in_template`.
pub fn in_template(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        if tok.tag_id == res::TAG_TEMPLATE {
            return super::in_head::in_head(tree, tkz, tok);
        }
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    match tok.tag_id {
        res::TAG__TEXT
        | res::TAG__EM_COMMENT
        | res::TAG__PROCESSINGINSTRUCTION
        | res::TAG__EM_DOCTYPE => super::in_body::in_body(tree, tkz, tok),
        res::TAG_BASE
        | res::TAG_BASEFONT
        | res::TAG_BGSOUND
        | res::TAG_LINK
        | res::TAG_META
        | res::TAG_NOFRAMES
        | res::TAG_SCRIPT
        | res::TAG_STYLE
        | res::TAG_TEMPLATE
        | res::TAG_TITLE => super::in_head::in_head(tree, tkz, tok),
        res::TAG_CAPTION | res::TAG_COLGROUP | res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD => {
            switch_to(tree, Mode::InTable)
        }
        res::TAG_COL => switch_to(tree, Mode::InColumnGroup),
        res::TAG_TR => switch_to(tree, Mode::InTableBody),
        res::TAG_TD | res::TAG_TH => switch_to(tree, Mode::InRow),
        res::TAG__END_OF_FILE => end_of_file(tree, tok),
        _ => switch_to(tree, Mode::InBody),
    }
}
