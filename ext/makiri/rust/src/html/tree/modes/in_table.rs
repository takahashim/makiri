//! The "in table" insertion mode (`insertion_mode/in_table.c`).

use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::Tokenizer;
use crate::html::tree::compat::{self, Fix};
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_clear_stack_back_to_table_context`.
fn clear_stack_back_to_table_context(tree: &mut TreeBuilder<'_>) {
    while let Some(c) = tree.current_node() {
        if matches!(
            tree.tag(c),
            res::TAG_TABLE | res::TAG_TEMPLATE | res::TAG_HTML
        ) && tree.ns(c) == res::NS_HTML
        {
            return;
        }
        tree.open_pop();
    }
}

/// `lxb_html_tree_insertion_mode_in_table_text_open`.
fn text_open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    let Some(node) = tree.current_node() else {
        return anything_else(tree, tkz, tok);
    };
    /* The Standard's list has `template` too; Lexbor's does not (P4). */
    let listed = match tree.tag(node) {
        res::TAG_TABLE | res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD | res::TAG_TR => true,
        res::TAG_TEMPLATE => compat::fixed(Fix::P4),
        _ => false,
    };
    if tree.ns(node) == res::NS_HTML && listed {
        tree.pending.list.clear();
        tree.pending.bytes.clear();
        tree.pending.have_non_ws = false;
        tree.original_mode = tree.mode;
        tree.mode = Mode::InTableText;
        return false;
    }
    anything_else(tree, tkz, tok)
}

/// Insert an HTML element for the token; `false` when that failed and the
/// parse is aborted.
fn insert(
    tree: &mut TreeBuilder<'_>,
    tkz: &Tokenizer<'_, '_>,
    tok: &Tok,
) -> Option<crate::html::dom::NodeId> {
    match tree.insert_html_element(tkz, tok) {
        Ok(el) => Some(el),
        Err(_) => {
            tree.abort_with(TreeStatus::Oom);
            None
        }
    }
}

/// `lxb_html_tree_insertion_mode_in_table_caption`.
fn caption(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    clear_stack_back_to_table_context(tree);
    if let Err(st) = tree.afe_push_marker() {
        return tree.abort_with(st);
    }
    if insert(tree, tkz, tok).is_none() {
        return true;
    }
    tree.mode = Mode::InCaption;
    true
}

/// `lxb_html_tree_insertion_mode_in_table_colgroup`.
fn colgroup(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    clear_stack_back_to_table_context(tree);
    if insert(tree, tkz, tok).is_none() {
        return true;
    }
    tree.mode = Mode::InColumnGroup;
    true
}

/// `lxb_html_tree_insertion_mode_in_table_col`.
fn col(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>) -> bool {
    clear_stack_back_to_table_context(tree);
    if insert(tree, tkz, &Tok::fake(res::TAG_COLGROUP)).is_none() {
        return true;
    }
    tree.mode = Mode::InColumnGroup;
    false
}

/// `lxb_html_tree_insertion_mode_in_table_tbtfth`.
fn tbtfth(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    clear_stack_back_to_table_context(tree);
    if insert(tree, tkz, tok).is_none() {
        return true;
    }
    tree.mode = Mode::InTableBody;
    true
}

/// `lxb_html_tree_insertion_mode_in_table_tdthtr`.
fn tdthtr(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>) -> bool {
    clear_stack_back_to_table_context(tree);
    if insert(tree, tkz, &Tok::fake(res::TAG_TBODY)).is_none() {
        return true;
    }
    tree.mode = Mode::InTableBody;
    false
}

/// `lxb_html_tree_insertion_mode_in_table_table`.
fn table(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    let Some(node) = tree.element_in_scope(res::TAG_TABLE, res::NS_HTML, rt::CAT_SCOPE_TABLE)
    else {
        return true;
    };
    tree.open_pop_until_node(node, true);
    tree.reset_insertion_mode_appropriately();
    false
}

/// `lxb_html_tree_insertion_mode_in_table_table_closed`.
fn table_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    let Some(node) = tree.element_in_scope(res::TAG_TABLE, res::NS_HTML, rt::CAT_SCOPE_TABLE)
    else {
        tree.parse_error(tok, E::Unclto);
        return true;
    };
    tree.open_pop_until_node(node, true);
    tree.reset_insertion_mode_appropriately();
    true
}

/// `lxb_html_tree_insertion_mode_in_table_input`.
fn input(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    let hidden = tok.has_attrs
        && tkz.token.attrs.iter().any(|a| {
            a.name == Some(res::ATTR_TYPE)
                && tkz
                    .attr_value(a)
                    .is_some_and(|v| v.eq_ignore_ascii_case(b"hidden"))
        });
    if !hidden {
        return anything_else(tree, tkz, tok);
    }
    tree.parse_error(tok, E::Unto);
    let Some(el) = insert(tree, tkz, tok) else {
        return true;
    };
    tree.open_pop_until_node(el, true);
    tree.acknowledge_token_self_closing(tok);
    true
}

/// `lxb_html_tree_insertion_mode_in_table_form`.
fn form(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    let in_template = tree.form_in_template();
    if !in_template && tree.form.is_some() {
        return true;
    }
    /* Lexbor (and the Standard before whatwg/html#12257) ignores the token
     * whenever a template is open; the Standard now inserts it there,
     * without the form element pointer (docs/parser_differences.ja.md P2). */
    if in_template && !compat::fixed(Fix::P2) {
        return true;
    }
    let Some(el) = insert(tree, tkz, tok) else {
        return true;
    };
    if !in_template {
        tree.form = Some(el);
    }
    tree.open_pop_until_node(el, true);
    true
}

/// `lxb_html_tree_insertion_mode_in_table_anything_else`: "in body" with
/// foster parenting, whose answer Lexbor did not pass on.
pub fn anything_else(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    tree.foster_parenting = true;
    let done = super::in_body::in_body(tree, tkz, tok);
    if tree.status != TreeStatus::Ok {
        return tree.process_abort();
    }
    tree.foster_parenting = false;
    /* "In body" asks for its token back only for `<image>`, renamed `<img>`,
     * which the current mode reprocesses: this one, fostering it again. */
    done || !compat::fixed(Fix::P14)
}

/// `lxb_html_tree_insertion_mode_in_table`.
pub fn in_table(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_TABLE => table_closed(tree, tok),
            res::TAG_BODY
            | res::TAG_CAPTION
            | res::TAG_COL
            | res::TAG_COLGROUP
            | res::TAG_HTML
            | res::TAG_TBODY
            | res::TAG_TD
            | res::TAG_TFOOT
            | res::TAG_TH
            | res::TAG_THEAD
            | res::TAG_TR => {
                tree.parse_error(tok, E::Unclto);
                true
            }
            res::TAG_TEMPLATE => super::in_head::in_head(tree, tkz, tok),
            _ => anything_else(tree, tkz, tok),
        };
    }
    match tok.tag_id {
        res::TAG__TEXT => text_open(tree, tkz, tok),
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).is_err() {
                return tree.abort_with(TreeStatus::Oom);
            }
            true
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree.insert_processing_instruction(tkz, tok, None).is_err() {
                return tree.process_abort();
            }
            true
        }
        res::TAG__EM_DOCTYPE => {
            tree.parse_error(tok, E::Dotointamo);
            true
        }
        res::TAG_CAPTION => caption(tree, tkz, tok),
        res::TAG_COLGROUP => colgroup(tree, tkz, tok),
        res::TAG_COL => col(tree, tkz),
        res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD => tbtfth(tree, tkz, tok),
        res::TAG_TD | res::TAG_TH | res::TAG_TR => tdthtr(tree, tkz),
        res::TAG_TABLE => table(tree, tok),
        res::TAG_STYLE | res::TAG_SCRIPT | res::TAG_TEMPLATE => {
            super::in_head::in_head(tree, tkz, tok)
        }
        res::TAG_INPUT => input(tree, tkz, tok),
        res::TAG_FORM => form(tree, tkz, tok),
        res::TAG__END_OF_FILE => super::in_body::in_body(tree, tkz, tok),
        _ => anything_else(tree, tkz, tok),
    }
}
