//! The "in head" insertion mode (`insertion_mode/in_head.c`).

use crate::html::res::names as res;
use crate::html::tokenizer::{TextContent, Tokenizer};
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Found, Mode, OrAbort, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_in_head`.
pub fn in_head(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return closed(tree, tkz, tok);
    }
    open(tree, tkz, tok)
}

fn open(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
        res::TAG__EM_COMMENT => {
            let _ = tree.insert_comment(tkz, tok, None).or_abort(tree);
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            let _ = tree.insert_processing_instruction(tkz, None).or_abort(tree);
        }
        res::TAG__EM_DOCTYPE => tree.parse_error(tok, E::Dotoinhemo),
        res::TAG_HTML => return super::in_body::in_body(tree, tkz, tok),
        res::TAG_BASE | res::TAG_BASEFONT | res::TAG_BGSOUND | res::TAG_LINK | res::TAG_META => {
            if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
                return true;
            }
            tree.open_pop();
            tree.acknowledge_token_self_closing(tok);
        }
        res::TAG_TITLE => {
            let _ = tree.generic_rcdata_parsing(tkz, tok).or_abort(tree);
        }
        res::TAG_NOSCRIPT => {
            let made = if tree.scripting {
                tree.generic_rawtext_parsing(tkz, tok)
            } else {
                let el = tree.insert_html_element(tkz, tok);
                tree.mode = Mode::InHeadNoscript;
                el
            };
            let _ = made.or_abort(tree);
        }
        res::TAG_NOFRAMES | res::TAG_STYLE => {
            let _ = tree.generic_rawtext_parsing(tkz, tok).or_abort(tree);
        }
        res::TAG_SCRIPT => return script(tree, tkz, tok),
        res::TAG_TEMPLATE => return template(tree, tkz, tok),
        res::TAG_HEAD => tree.parse_error(tok, E::Hetoinhemo),
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                let ws = tok.split_ws_begin(tkz);
                if ws.text_start != ws.text_end
                    && tree.insert_character(tkz, &ws).or_abort(tree).is_none()
                {
                    return true;
                }
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            return anything_else(tree);
        }
    }
    true
}

fn closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    match tok.tag_id {
        res::TAG_HEAD => {
            tree.open_pop();
            tree.mode = Mode::AfterHead;
        }
        res::TAG_BODY | res::TAG_HTML | res::TAG_BR => return anything_else(tree),
        res::TAG_TEMPLATE => return template_closed(tree, tkz, tok),
        _ => tree.parse_error(tok, E::Uncltoinhemo),
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_head_script`.
fn script(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    let Some((pos, ipos)) = tree.appropriate_place(None) else {
        return tree.abort_with(TreeStatus::Error);
    };
    let Some(el) = tree
        .create_element_for_token(tkz, tok, res::NS_HTML, None)
        .or_abort(tree)
    else {
        return true;
    };
    if tree.open_push(el).or_abort(tree).is_none() {
        return true;
    }
    tree.insert_node(pos, el, ipos);
    tkz.enter_text_content(TextContent::ScriptData, tok.tag_id);
    tree.original_mode = tree.mode;
    tree.mode = Mode::Text;
    true
}

/// `lxb_html_tree_insertion_mode_in_head_template`.
fn template(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    if tree.afe_push_marker().or_abort(tree).is_none() {
        return true;
    }
    tree.frameset_ok = false;
    tree.mode = Mode::InTemplate;
    let _ = tree.template_modes_push(Mode::InTemplate).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_head_template_closed`.
fn template_closed(tree: &mut TreeBuilder<'_>, _tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .open_find_reverse(res::TAG_TEMPLATE, res::NS_HTML)
        .is_none()
    {
        tree.parse_error(tok, E::Tecltowiopinhemo);
        return true;
    }
    tree.generate_all_implied_end_tags_thoroughly(res::TAG__UNDEF, res::NS__UNDEF);
    let is_template = tree.current_is(res::TAG_TEMPLATE);
    if !is_template {
        tree.parse_error(tok, E::Teelisnocuinhemo);
    }
    tree.open_pop_until_tag_id(res::TAG_TEMPLATE, res::NS_HTML, Found::Popped);
    tree.afe_up_to_last_marker();
    tree.template_modes_pop();
    tree.reset_insertion_mode_appropriately();
    true
}

/// `lxb_html_tree_insertion_mode_in_head_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>) -> bool {
    tree.open_pop();
    tree.mode = Mode::AfterHead;
    false
}
