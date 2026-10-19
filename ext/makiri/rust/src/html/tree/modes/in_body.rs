//! The "in body" insertion mode (`insertion_mode/in_body.c`), with Lexbor's
//! two helper modes that skip the newline after `<pre>`, `<listing>` and
//! `<textarea>`.

use crate::compat::{self, Fix};
use crate::falloc::try_vec_with_capacity;
use crate::html::dom::CompatMode;
use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::{flags, TextContent, Tokenizer};
use crate::html::tree::actions::is_category;
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{is_ws, AttrAdjust, Mode, OrAbort, Tok, TreeBuilder, TreeStatus};

/// `lxb_html_tree_insertion_mode_in_body_skip_new_line`.
pub fn skip_new_line(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    tree.mode = tree.original_mode;
    if tok.tag_id != res::TAG__TEXT {
        return false;
    }
    tok.skip_one_newline_begin(tkz);
    tok.text_start == tok.text_end
}

/// `lxb_html_tree_insertion_mode_in_body_skip_new_line_textarea`: the
/// Standard goes on in "text", which inserts the contents without
/// reconstructing formatting elements; Lexbor went back to the mode the
/// `<textarea>` came in.
pub fn skip_new_line_textarea(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    let done = skip_new_line(tree, tkz, tok);
    if compat::fixed(Fix::P12) {
        tree.mode = Mode::Text;
    }
    done
}

/// `lxb_html_tree_insertion_mode_in_body_text`.
fn text(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    let data = tok.text(tkz);
    let r = if tok.null_count != 0 {
        tree.parse_error(tok, E::Nuch);
        let Some(mut dropped) = try_vec_with_capacity::<u8>(data.len()) else {
            return tree.abort_with(TreeStatus::Oom);
        };
        dropped.extend(data.iter().copied().filter(|&c| c != 0));
        /* Can be empty only when every byte was a NUL. */
        if dropped.is_empty() {
            return true;
        }
        text_append(tree, tkz, &dropped)
    } else {
        if data.is_empty() {
            return true;
        }
        text_append(tree, tkz, data)
    };
    let _ = r.or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_text_append`.
pub fn text_append(
    tree: &mut TreeBuilder<'_>,
    tkz: &Tokenizer<'_, '_>,
    data: &[u8],
) -> Result<(), TreeStatus> {
    tree.afe_reconstruct(tkz)?;
    if tree.frameset_ok && data.iter().any(|&c| !is_ws(c)) {
        tree.frameset_ok = false;
    }
    tree.insert_character_for_data(data)
}

/// `lxb_html_tree_insertion_mode_in_body_html`.
fn html(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    if tree.open_find(res::TAG_TEMPLATE, res::NS_HTML).is_some() {
        return true;
    }
    let Some(html) = tree.open_first() else {
        return true;
    };
    let ns = tree.ns(html);
    if tree
        .append_attributes(tkz, html, tok, ns, None)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_blmnst`.
fn blmnst(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    super::in_head::in_head(tree, tkz, tok)
}

/// `lxb_html_tree_insertion_mode_in_body_body`.
fn body(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    let Some(node) = tree.open_get(1).filter(|&n| tree.tag(n) == res::TAG_BODY) else {
        return true;
    };
    if tree
        .open_find_reverse(res::TAG_TEMPLATE, res::NS_HTML)
        .is_some()
    {
        return true;
    }
    tree.frameset_ok = false;
    let ns = tree.ns(node);
    if tree
        .append_attributes(tkz, node, tok, ns, None)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_frameset`.
fn frameset(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    let Some(node) = tree.open_get(1).filter(|&n| tree.tag(n) == res::TAG_BODY) else {
        return true;
    };
    if !tree.frameset_ok {
        return true;
    }
    /* `lxb_html_tree_node_delete_deep` */
    tree.doc.detach(node);
    if let Some(html) = tree.open_get(0) {
        tree.open_pop_until_node(html, false);
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.mode = Mode::InFrameset;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_eof`.
fn eof(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tree.in_template_mode() {
        return super::in_template::in_template(tree, tkz, tok);
    }
    if !tree.check_scope_element() {
        tree.parse_error(tok, E::Baenopeliswr);
    }
    /* `lxb_html_tree_stop_parsing` */
    true
}

/// `lxb_html_tree_insertion_mode_in_body_body_closed`.
fn body_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_BODY, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Noboelinsc);
        return true;
    }
    if !tree.check_scope_element() {
        tree.parse_error(tok, E::Opeliswr);
    }
    tree.mode = Mode::AfterBody;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_html_closed`.
fn html_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_BODY, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Noboelinsc);
        return true;
    }
    if !tree.check_scope_element() {
        tree.parse_error(tok, E::Opeliswr);
    }
    tree.mode = Mode::AfterBody;
    false
}

/// Close a `<p>` in button scope, if there is one.
fn close_p_in_button_scope(tree: &mut TreeBuilder<'_>, tok: &Tok) {
    if tree
        .element_in_scope(res::TAG_P, res::NS_HTML, rt::CAT_SCOPE_BUTTON)
        .is_some()
    {
        tree.close_p_element(tok);
    }
}

/// Reconstruct the active formatting elements; `false` when that failed
/// and the parse is aborted.
fn reconstruct(tree: &mut TreeBuilder<'_>, tkz: &Tokenizer<'_, '_>) -> bool {
    tree.afe_reconstruct(tkz).or_abort(tree).is_some()
}

/// `lxb_html_tree_insertion_mode_in_body_abcdfhmnopsu`.
fn abcdfhmnopsu(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_h123456`.
fn h123456(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    if let Some(node) = tree.current_node() {
        if super::super::stacks::is_h123456(tree.tag(node)) {
            tree.parse_error(tok, E::Unelinopelst);
            tree.open_pop();
        }
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_pre_listing`.
fn pre_listing(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.original_mode = tree.mode;
    tree.mode = Mode::InBodySkipNewLine;
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_form`.
fn form(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    let in_template = tree.form_in_template();
    if tree.form.is_some() && !in_template {
        tree.parse_error(tok, E::Unto);
        return true;
    }
    close_p_in_button_scope(tree, tok);
    let Some(el) = tree.insert_html_element(tkz, tok).or_abort(tree) else {
        return true;
    };
    if !in_template {
        tree.form = Some(el);
    }
    true
}

/// The loop shared by `li` and `dd`/`dt`: close the nearest open element
/// of `tags`, unless a special element other than address, div and p is
/// met first.
fn close_list_item(tree: &mut TreeBuilder<'_>, tok: &Tok, tags: &[usize]) {
    tree.frameset_ok = false;
    let mut idx = tree.open_len();
    while idx != 0 {
        idx -= 1;
        let node = tree.open_at(idx);
        if let Some(&tag) = tags.iter().find(|&&t| tree.node_is(node, t)) {
            tree.close_element(tok, tag, Some(tag), E::Unelinopelst);
            break;
        }
        if is_category(tree.tag(node), tree.ns(node), rt::CAT_SPECIAL)
            && !tree.node_is(node, res::TAG_ADDRESS)
            && !tree.node_is(node, res::TAG_DIV)
            && !tree.node_is(node, res::TAG_P)
        {
            break;
        }
    }
}

/// `lxb_html_tree_insertion_mode_in_body_li`.
fn li(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_list_item(tree, tok, &[res::TAG_LI]);
    close_p_in_button_scope(tree, tok);
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_dd_dt`.
fn dd_dt(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_list_item(tree, tok, &[res::TAG_DD, res::TAG_DT]);
    close_p_in_button_scope(tree, tok);
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_plaintext`.
fn plaintext(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tkz.enter_plaintext();
    true
}

/// `lxb_html_tree_insertion_mode_in_body_button`.
fn button(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_BUTTON, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.parse_error(tok, E::Unto);
        tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
        tree.open_pop_until_tag_id(res::TAG_BUTTON, res::NS_HTML, true);
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_abcdfhlmnopsu_closed`.
fn abcdfhlmnopsu_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.close_element(tok, tok.tag_id, None, E::Unelinopelst);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_form_closed`, with the template
/// test the start tag uses (`form_in_template`).
fn form_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if !tree.form_in_template() {
        let node = tree.form.take();
        let Some(node) = node else {
            tree.parse_error(tok, E::Unclto);
            return true;
        };
        let Some(node) = tree.element_in_scope_by_node(node, rt::CAT_SCOPE) else {
            tree.parse_error(tok, E::Unclto);
            return true;
        };
        tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
        if tree.current_node() != Some(node) {
            tree.parse_error(tok, E::Unelinopelst);
        }
        tree.open_remove_by_node(node);
        return true;
    }
    if tree
        .element_in_scope(res::TAG_FORM, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.close_element(tok, res::TAG_FORM, None, E::Unelinopelst);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_p_closed`.
fn p_closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_P, res::NS_HTML, rt::CAT_SCOPE_BUTTON)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        let fake = Tok::fake(res::TAG_P);
        if tree
            .insert_html_element(tkz, &fake)
            .or_abort(tree)
            .is_none()
        {
            return true;
        }
    }
    tree.close_p_element(tok);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_li_closed`.
fn li_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_LI, res::NS_HTML, rt::CAT_SCOPE_LIST_ITEM)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.close_element(tok, res::TAG_LI, Some(res::TAG_LI), E::Unelinopelst);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_dd_dt_closed`.
fn dd_dt_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.close_element(tok, tok.tag_id, Some(tok.tag_id), E::Unelinopelst);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_h123456_closed`.
fn h123456_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree.element_in_scope_h123456().is_none() {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
    let is = tree.current_is(tok.tag_id);
    if !is {
        tree.parse_error(tok, E::Unelinopelst);
    }
    tree.open_pop_until_h123456();
    true
}

/// `lxb_html_tree_insertion_mode_in_body_a`.
fn a(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if let Some((node, _)) = tree.afe_between_last_marker(tok.tag_id) {
        tree.parse_error(tok, E::Unelinacfost);
        if tree.adoption_agency_algorithm(tok).or_abort(tree).is_none() {
            return true;
        }
        tree.afe_remove_by_node(node);
        tree.open_remove_by_node(node);
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    let Some(el) = tree.insert_html_element(tkz, tok).or_abort(tree) else {
        return true;
    };
    /* Lexbor ignores a failed push here. */
    let _ = tree.afe_push_with_check_dupl(el);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_bcefistu`.
fn bcefistu(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    let Some(el) = tree.insert_html_element(tkz, tok).or_abort(tree) else {
        return true;
    };
    let _ = tree.afe_push_with_check_dupl(el);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_nobr`.
fn nobr(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    if tree
        .element_in_scope(res::TAG_NOBR, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.parse_error(tok, E::Unelinsc);
        let Some(run_any_other_end_tag) = tree.adoption_agency_algorithm(tok).or_abort(tree) else {
            return true;
        };
        if run_any_other_end_tag {
            anything_else_closed(tree, tok);
        }
        if !reconstruct(tree, tkz) {
            return true;
        }
    }
    let Some(el) = tree.insert_html_element(tkz, tok).or_abort(tree) else {
        return true;
    };
    let _ = tree.afe_push_with_check_dupl(el);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_abcefinstu_closed`.
fn abcefinstu_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    let Some(run_any_other_end_tag) = tree.adoption_agency_algorithm(tok).or_abort(tree) else {
        return true;
    };
    if run_any_other_end_tag {
        return anything_else_closed(tree, tok);
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_amo`.
fn amo(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    if tree.afe_push_marker().or_abort(tree).is_none() {
        return true;
    }
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_amo_closed`.
fn amo_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(tok.tag_id, res::NS_HTML, rt::CAT_SCOPE)
        .is_none()
    {
        tree.parse_error(tok, E::Unclto);
        return true;
    }
    tree.close_element(tok, tok.tag_id, None, E::Unelinopelst);
    tree.afe_up_to_last_marker();
    true
}

/// `lxb_html_tree_insertion_mode_in_body_table`.
fn table(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    /* LXB_DOM_DOCUMENT_CMODE_QUIRKS */
    if tree.doc.compat_mode() != CompatMode::Quirks {
        close_p_in_button_scope(tree, tok);
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.frameset_ok = false;
    tree.mode = Mode::InTable;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_abeikw`.
fn abeikw(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.open_pop();
    tree.acknowledge_token_self_closing(tok);
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_br_closed`: `</br>` is `<br>`,
/// without attributes.
fn br_closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    tok.flags &= !flags::CLOSE;
    tok.has_attrs = false;
    abeikw(tree, tkz, tok)
}

/// `lxb_html_tree_insertion_mode_in_body_input`.
fn input(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if is_fragment_element(tree, res::TAG_SELECT, res::NS_HTML) {
        tree.parse_error(tok, E::Frpaseincopain);
        return true;
    }
    if let Some(node) = tree.element_in_scope(res::TAG_SELECT, res::NS_HTML, rt::CAT_SCOPE) {
        tree.parse_error(tok, E::Seinsc);
        tree.open_pop_until_node(node, true);
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    let Some(el) = tree.insert_html_element(tkz, tok).or_abort(tree) else {
        return true;
    };
    tree.open_pop();
    tree.acknowledge_token_self_closing(tok);
    /* `lxb_dom_element_attr_is_exist(element, "type")`: by local name or by
     * qualified name; the value compared exactly. */
    let found = tree.attr_is_exist(el, b"type");
    let d = &*tree.doc;
    let hidden = match found {
        /* The Standard compares the value ASCII case-insensitively; Lexbor
         * exactly. */
        Some(x) if compat::fixed(Fix::P13) => d
            .attr_value(x)
            .is_some_and(|v| v.eq_ignore_ascii_case(b"hidden")),
        Some(x) => !tree.valueless.contains(x) && d.attr_value(x) == Some(&b"hidden"[..]),
        None => false,
    };
    if !hidden {
        tree.frameset_ok = false;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_pst`.
fn pst(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.open_pop();
    tree.acknowledge_token_self_closing(tok);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_hr`.
fn hr(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    if tree
        .element_in_scope(res::TAG_SELECT, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
        if tree.element_in_scope_option_optgroup().is_some() {
            tree.parse_error(tok, E::Hrpaseopopinsc);
        }
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.open_pop();
    tree.acknowledge_token_self_closing(tok);
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_image`.
fn image(tree: &mut TreeBuilder<'_>, tok: &mut Tok) -> bool {
    tree.parse_error(tok, E::Unto);
    tok.tag_id = res::TAG_IMG;
    false
}

/// `lxb_html_tree_insertion_mode_in_body_textarea`.
fn textarea(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tkz.enter_text_content(TextContent::Rcdata, res::TAG_TEXTAREA);
    tree.frameset_ok = false;
    tree.original_mode = tree.mode;
    tree.mode = Mode::InBodySkipNewLineTextarea;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_xmp`.
fn xmp(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    close_p_in_button_scope(tree, tok);
    if !reconstruct(tree, tkz) {
        return true;
    }
    tree.frameset_ok = false;
    if tree
        .generic_rawtext_parsing(tkz, tok)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_iframe`.
fn iframe(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    tree.frameset_ok = false;
    if tree
        .generic_rawtext_parsing(tkz, tok)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_noembed`.
fn noembed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .generic_rawtext_parsing(tkz, tok)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_is_fragment_element`.
fn is_fragment_element(tree: &TreeBuilder<'_>, tag: usize, ns: usize) -> bool {
    tree.fragment
        .is_some_and(|f| tree.tag(f) == tag && tree.ns(f) == ns)
}

/// `lxb_html_tree_insertion_mode_in_body_select`.
fn select(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if is_fragment_element(tree, res::TAG_SELECT, res::NS_HTML) {
        tree.parse_error(tok, E::Frpaseincopase);
        return true;
    }
    if let Some(node) = tree.element_in_scope(res::TAG_SELECT, res::NS_HTML, rt::CAT_SCOPE) {
        tree.parse_error(tok, E::Seinsc);
        tree.open_pop_until_node(node, true);
        return true;
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    if tree.insert_html_element(tkz, tok).or_abort(tree).is_none() {
        return true;
    }
    tree.frameset_ok = false;
    true
}

/// `lxb_html_tree_insertion_mode_in_body_option`.
fn option(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_SELECT, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.generate_implied_end_tags(res::TAG_OPTGROUP, res::NS_HTML);
        if tree
            .element_in_scope(res::TAG_OPTION, res::NS_HTML, rt::CAT_SCOPE)
            .is_some()
        {
            tree.parse_error(tok, E::Oppaopinsc);
        }
    } else if tree.current_is(res::TAG_OPTION) {
        tree.open_pop();
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_optgroup`.
fn optgroup(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_SELECT, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
        if tree.element_in_scope_option_optgroup().is_some() {
            tree.parse_error(tok, E::Oppaopopinsc);
        }
    } else if tree.current_is(res::TAG_OPTION) {
        tree.open_pop();
    }
    if !reconstruct(tree, tkz) {
        return true;
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_rbrtc`.
fn rbrtc(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_RUBY, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF);
    }
    let is = tree.current_is(res::TAG_RUBY);
    if !is {
        tree.parse_error(tok, E::Mielinopelst);
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_rprt`.
fn rprt(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if tree
        .element_in_scope(res::TAG_RUBY, res::NS_HTML, rt::CAT_SCOPE)
        .is_some()
    {
        tree.generate_implied_end_tags(res::TAG_RTC, res::NS_HTML);
    }
    let is = tree
        .current_node()
        .is_some_and(|c| tree.node_is(c, res::TAG_RTC) || tree.node_is(c, res::TAG_RUBY));
    if !is {
        tree.parse_error(tok, E::Mielinopelst);
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_math` and `..._svg`.
fn foreign(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &Tok,
    ns: usize,
    adjust: AttrAdjust,
) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    let made = tree.insert_foreign_element(tkz, tok, ns, Some(adjust));
    if made.or_abort(tree).is_none() {
        return true;
    }
    if tok.flags & flags::CLOSE_SELF != 0 {
        tree.open_pop();
        tree.acknowledge_token_self_closing(tok);
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !reconstruct(tree, tkz) {
        return true;
    }
    tree.insert_html_element(tkz, tok).or_abort(tree);
    true
}

/// `lxb_html_tree_insertion_mode_in_body_noscript`.
fn noscript(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    if !tree.scripting {
        return anything_else(tree, tkz, tok);
    }
    if tree
        .generic_rawtext_parsing(tkz, tok)
        .or_abort(tree)
        .is_none()
    {
        return true;
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body_anything_else_closed`.
pub fn anything_else_closed(tree: &mut TreeBuilder<'_>, tok: &Tok) -> bool {
    let mut len = tree.open_len();
    while len != 0 {
        len -= 1;
        let node = tree.open_at(len);
        if tree.node_is(node, tok.tag_id) {
            tree.generate_implied_end_tags(tok.tag_id, res::NS_HTML);
            if Some(node) != tree.current_node() {
                tree.parse_error(tok, E::Unelinopelst);
            }
            tree.open_pop_until_node(node, true);
            return true;
        }
        if is_category(tree.tag(node), tree.ns(node), rt::CAT_SPECIAL) {
            tree.parse_error(tok, E::Unclto);
            return true;
        }
    }
    true
}

/// `lxb_html_tree_insertion_mode_in_body`.
pub fn in_body(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_TEMPLATE => blmnst(tree, tkz, tok),
            res::TAG_BODY => body_closed(tree, tok),
            res::TAG_HTML => html_closed(tree, tok),
            res::TAG_ADDRESS
            | res::TAG_ARTICLE
            | res::TAG_ASIDE
            | res::TAG_BLOCKQUOTE
            | res::TAG_BUTTON
            | res::TAG_CENTER
            | res::TAG_DETAILS
            | res::TAG_DIALOG
            | res::TAG_DIR
            | res::TAG_DIV
            | res::TAG_DL
            | res::TAG_FIELDSET
            | res::TAG_FIGCAPTION
            | res::TAG_FIGURE
            | res::TAG_FOOTER
            | res::TAG_HEADER
            | res::TAG_HGROUP
            | res::TAG_LISTING
            | res::TAG_MAIN
            | res::TAG_MENU
            | res::TAG_NAV
            | res::TAG_OL
            | res::TAG_PRE
            | res::TAG_SEARCH
            | res::TAG_SECTION
            | res::TAG_SELECT
            | res::TAG_SUMMARY
            | res::TAG_UL => abcdfhlmnopsu_closed(tree, tok),
            res::TAG_FORM => form_closed(tree, tok),
            res::TAG_P => p_closed(tree, tkz, tok),
            res::TAG_LI => li_closed(tree, tok),
            res::TAG_DD | res::TAG_DT => dd_dt_closed(tree, tok),
            res::TAG_H1 | res::TAG_H2 | res::TAG_H3 | res::TAG_H4 | res::TAG_H5 | res::TAG_H6 => {
                h123456_closed(tree, tok)
            }
            res::TAG_A
            | res::TAG_B
            | res::TAG_BIG
            | res::TAG_CODE
            | res::TAG_EM
            | res::TAG_FONT
            | res::TAG_I
            | res::TAG_NOBR
            | res::TAG_S
            | res::TAG_SMALL
            | res::TAG_STRIKE
            | res::TAG_STRONG
            | res::TAG_TT
            | res::TAG_U => abcefinstu_closed(tree, tok),
            res::TAG_APPLET | res::TAG_MARQUEE | res::TAG_OBJECT => amo_closed(tree, tok),
            res::TAG_BR => br_closed(tree, tkz, tok),
            _ => anything_else_closed(tree, tok),
        };
    }
    match tok.tag_id {
        res::TAG__TEXT => text(tree, tkz, tok),
        res::TAG__EM_COMMENT => {
            if tree.insert_comment(tkz, tok, None).or_abort(tree).is_none() {
                return true;
            }
            true
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            if tree
                .insert_processing_instruction(tkz, None)
                .or_abort(tree)
                .is_none()
            {
                return true;
            }
            true
        }
        res::TAG__EM_DOCTYPE => {
            tree.parse_error(tok, E::Dotoinbomo);
            true
        }
        res::TAG_HTML => html(tree, tkz, tok),
        res::TAG_BASE
        | res::TAG_BASEFONT
        | res::TAG_BGSOUND
        | res::TAG_LINK
        | res::TAG_META
        | res::TAG_NOFRAMES
        | res::TAG_SCRIPT
        | res::TAG_STYLE
        | res::TAG_TEMPLATE
        | res::TAG_TITLE => blmnst(tree, tkz, tok),
        res::TAG_BODY => body(tree, tkz, tok),
        res::TAG_FRAMESET => frameset(tree, tkz, tok),
        res::TAG__END_OF_FILE => eof(tree, tkz, tok),
        res::TAG_ADDRESS
        | res::TAG_ARTICLE
        | res::TAG_ASIDE
        | res::TAG_BLOCKQUOTE
        | res::TAG_CENTER
        | res::TAG_DETAILS
        | res::TAG_DIALOG
        | res::TAG_DIR
        | res::TAG_DIV
        | res::TAG_DL
        | res::TAG_FIELDSET
        | res::TAG_FIGCAPTION
        | res::TAG_FIGURE
        | res::TAG_FOOTER
        | res::TAG_HEADER
        | res::TAG_HGROUP
        | res::TAG_MAIN
        | res::TAG_MENU
        | res::TAG_NAV
        | res::TAG_OL
        | res::TAG_P
        | res::TAG_SEARCH
        | res::TAG_SECTION
        | res::TAG_SUMMARY
        | res::TAG_UL => abcdfhmnopsu(tree, tkz, tok),
        res::TAG_H1 | res::TAG_H2 | res::TAG_H3 | res::TAG_H4 | res::TAG_H5 | res::TAG_H6 => {
            h123456(tree, tkz, tok)
        }
        res::TAG_PRE | res::TAG_LISTING => pre_listing(tree, tkz, tok),
        res::TAG_FORM => form(tree, tkz, tok),
        res::TAG_LI => li(tree, tkz, tok),
        res::TAG_DD | res::TAG_DT => dd_dt(tree, tkz, tok),
        res::TAG_PLAINTEXT => plaintext(tree, tkz, tok),
        res::TAG_BUTTON => button(tree, tkz, tok),
        res::TAG_A => a(tree, tkz, tok),
        res::TAG_B
        | res::TAG_BIG
        | res::TAG_CODE
        | res::TAG_EM
        | res::TAG_FONT
        | res::TAG_I
        | res::TAG_S
        | res::TAG_SMALL
        | res::TAG_STRIKE
        | res::TAG_STRONG
        | res::TAG_TT
        | res::TAG_U => bcefistu(tree, tkz, tok),
        res::TAG_NOBR => nobr(tree, tkz, tok),
        res::TAG_APPLET | res::TAG_MARQUEE | res::TAG_OBJECT => amo(tree, tkz, tok),
        res::TAG_TABLE => table(tree, tkz, tok),
        res::TAG_AREA
        | res::TAG_BR
        | res::TAG_EMBED
        | res::TAG_IMG
        | res::TAG_KEYGEN
        | res::TAG_WBR => abeikw(tree, tkz, tok),
        res::TAG_INPUT => input(tree, tkz, tok),
        res::TAG_PARAM | res::TAG_SOURCE | res::TAG_TRACK => pst(tree, tkz, tok),
        res::TAG_HR => hr(tree, tkz, tok),
        res::TAG_IMAGE => image(tree, tok),
        res::TAG_TEXTAREA => textarea(tree, tkz, tok),
        res::TAG_XMP => xmp(tree, tkz, tok),
        res::TAG_IFRAME => iframe(tree, tkz, tok),
        res::TAG_NOEMBED => noembed(tree, tkz, tok),
        res::TAG_NOSCRIPT => noscript(tree, tkz, tok),
        res::TAG_SELECT => select(tree, tkz, tok),
        res::TAG_OPTION => option(tree, tkz, tok),
        res::TAG_OPTGROUP => optgroup(tree, tkz, tok),
        res::TAG_RB | res::TAG_RTC => rbrtc(tree, tkz, tok),
        res::TAG_RP | res::TAG_RT => rprt(tree, tkz, tok),
        res::TAG_MATH => foreign(tree, tkz, tok, res::NS_MATH, AttrAdjust::Mathml),
        res::TAG_SVG => foreign(tree, tkz, tok, res::NS_SVG, AttrAdjust::Svg),
        res::TAG_CAPTION
        | res::TAG_COL
        | res::TAG_COLGROUP
        | res::TAG_FRAME
        | res::TAG_HEAD
        | res::TAG_TBODY
        | res::TAG_TD
        | res::TAG_TFOOT
        | res::TAG_TH
        | res::TAG_THEAD
        | res::TAG_TR => {
            tree.parse_error(tok, E::Unto);
            true
        }
        _ => anything_else(tree, tkz, tok),
    }
}
