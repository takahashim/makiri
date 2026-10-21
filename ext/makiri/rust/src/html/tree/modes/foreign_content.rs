//! The rules for parsing tokens in foreign content
//! (`insertion_mode/foreign_content.c`).

use crate::compat::{self, Fix};
use crate::falloc::try_vec_with_capacity;
use crate::html::res::names as res;
use crate::html::tokenizer::{flags, Tokenizer};
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{is_ws, AttrAdjust, Found, OrAbort, Tok, TreeBuilder, TreeStatus};

/// U+FFFD in UTF-8.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// `..._foreign_content_anything_else_closed`.
fn anything_else_closed(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tree.open_len() == 0 {
        return tree.run(tree.mode, tkz, tok);
    }
    let mut idx = tree.open_len() - 1;
    if idx > 0
        && tree
            .current_node()
            .is_some_and(|n| tree.tag(n) != tok.tag_id)
    {
        tree.parse_error(tok, E::Unelinopelst);
    }
    while idx != 0 {
        let Some(node) = tree.open_get(idx) else {
            break;
        };
        if tree.tag(node) == tok.tag_id {
            tree.open_pop_until_node(node, Found::Popped);
            return true;
        }
        idx -= 1;
        if tree
            .open_get(idx)
            .is_some_and(|n| tree.ns(n) == res::NS_HTML)
        {
            break;
        }
    }
    tree.run(tree.mode, tkz, tok)
}

/// `..._foreign_content_script_closed`.
fn script_closed(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    let is = tree
        .current_node()
        .is_some_and(|n| tree.tag(n) == res::TAG_SCRIPT && tree.ns(n) == res::NS_SVG);
    if !is {
        return anything_else_closed(tree, tkz, tok);
    }
    tree.open_pop();
    true
}

/// `..._foreign_content_anything_else`.
fn anything_else(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    let Some(node) = tree.adjusted_current_node() else {
        return tree.abort_with(TreeStatus::Error);
    };
    let ns = tree.ns(node);
    let adjust = if ns == res::NS_MATH {
        Some(AttrAdjust::Mathml)
    } else if ns == res::NS_SVG {
        Some(AttrAdjust::Svg)
    } else if compat::fixed(Fix::P19) {
        /* The Standard adjusts foreign attributes in every namespace. */
        Some(AttrAdjust::Foreign)
    } else {
        None
    };
    let made = tree.insert_foreign_element(tkz, tok, ns, adjust);
    let Some(el) = made.or_abort(tree) else {
        return true;
    };
    if ns == res::NS_SVG {
        tree.adjust_svg_tag_name(el);
    }
    if tok.flags & flags::CLOSE_SELF == 0 {
        return true;
    }
    let current_svg = tree
        .current_node()
        .is_some_and(|n| tree.ns(n) == res::NS_SVG);
    if tok.tag_id == res::TAG_SCRIPT && current_svg {
        return script_closed(tree, tkz, tok);
    }
    tree.open_pop();
    true
}

/// `..._foreign_content_text`: U+0000 becomes U+FFFD. Lexbor's frameset-ok
/// test passes over every U+FFFD, and the Standard's only over the ones a
/// U+0000 became (P3 in `docs/parser_differences.ja.md`).
fn text(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    let data = tok.text(tkz);
    let replaced;
    let s: &[u8] = if tok.null_count != 0 {
        tree.parse_error(tok, E::Nuch);
        let Some(mut v) = try_vec_with_capacity::<u8>(data.len() + tok.null_count * 2) else {
            return tree.abort_with(TreeStatus::Oom);
        };
        for &c in data {
            if c == 0 {
                v.extend_from_slice(REPLACEMENT);
            } else {
                v.push(c);
            }
        }
        replaced = v;
        &replaced
    } else {
        data
    };
    if s.is_empty() {
        return true;
    }
    if tree.frameset_ok && compat::fixed(Fix::P3) {
        /* "Any other character token" sets frameset-ok to "not ok"; only a
         * U+0000 (the token the replacement came from) and whitespace leave
         * it. A U+FFFD written in the input is any other character. */
        if data.iter().any(|&c| c != 0 && !is_ws(c)) {
            tree.frameset_ok = false;
        }
    } else if tree.frameset_ok {
        let mut pos = 0;
        while pos < s.len() {
            if s[pos] == REPLACEMENT[0] {
                if s.len() - pos < REPLACEMENT.len()
                    || &s[pos..pos + REPLACEMENT.len()] != REPLACEMENT
                {
                    tree.frameset_ok = false;
                    break;
                }
                pos += REPLACEMENT.len();
                continue;
            }
            if !is_ws(s[pos]) {
                tree.frameset_ok = false;
                break;
            }
            pos += 1;
        }
    }
    let _ = tree.insert_character_for_data(s).or_abort(tree);
    true
}

/// `..._foreign_content_all`: a start tag (or `</p>`, `</br>`) that breaks
/// out of foreign content.
fn all(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    if tok.tag_id == res::TAG_FONT {
        let breaks = tok.attrs(tkz).iter().any(|a| {
            matches!(
                a.name,
                Some(res::ATTR_COLOR) | Some(res::ATTR_FACE) | Some(res::ATTR_SIZE)
            )
        });
        if !breaks {
            return anything_else(tree, tkz, tok);
        }
    }
    tree.parse_error(tok, E::Unto);
    while let Some(node) = tree.current_node() {
        if tree.mathml_text_integration_point(node)
            || tree.html_integration_point(node)
            || tree.ns(node) == res::NS_HTML
        {
            break;
        }
        tree.open_pop();
    }
    tree.run(tree.mode, tkz, tok)
}

/// `lxb_html_tree_insertion_mode_foreign_content`.
pub fn foreign_content(
    tree: &mut TreeBuilder<'_>,
    tkz: &mut Tokenizer<'_, '_>,
    tok: &mut Tok,
) -> bool {
    if tok.is_close() {
        return match tok.tag_id {
            res::TAG_SCRIPT => script_closed(tree, tkz, tok),
            res::TAG_P | res::TAG_BR => all(tree, tkz, tok),
            _ => anything_else_closed(tree, tkz, tok),
        };
    }
    match tok.tag_id {
        res::TAG__TEXT => text(tree, tkz, tok),
        res::TAG__EM_COMMENT => {
            let _ = tree.insert_comment(tkz, tok, None).or_abort(tree);
            true
        }
        res::TAG__PROCESSINGINSTRUCTION => {
            let _ = tree.insert_processing_instruction(tkz, None).or_abort(tree);
            true
        }
        res::TAG__EM_DOCTYPE => {
            tree.parse_error(tok, E::Dotofocomo);
            true
        }
        res::TAG_B
        | res::TAG_BIG
        | res::TAG_BLOCKQUOTE
        | res::TAG_BODY
        | res::TAG_BR
        | res::TAG_CENTER
        | res::TAG_CODE
        | res::TAG_DD
        | res::TAG_DIV
        | res::TAG_DL
        | res::TAG_DT
        | res::TAG_EM
        | res::TAG_EMBED
        | res::TAG_H1
        | res::TAG_H2
        | res::TAG_H3
        | res::TAG_H4
        | res::TAG_H5
        | res::TAG_H6
        | res::TAG_HEAD
        | res::TAG_HR
        | res::TAG_I
        | res::TAG_IMG
        | res::TAG_LI
        | res::TAG_LISTING
        | res::TAG_MENU
        | res::TAG_META
        | res::TAG_NOBR
        | res::TAG_OL
        | res::TAG_P
        | res::TAG_PRE
        | res::TAG_RUBY
        | res::TAG_S
        | res::TAG_SMALL
        | res::TAG_SPAN
        | res::TAG_STRONG
        | res::TAG_STRIKE
        | res::TAG_SUB
        | res::TAG_TABLE
        | res::TAG_TT
        | res::TAG_U
        | res::TAG_UL
        | res::TAG_VAR
        | res::TAG_FONT => all(tree, tkz, tok),
        /* In the Standard's list, missing from Lexbor's. */
        res::TAG_SUP if compat::fixed(Fix::P15) => all(tree, tkz, tok),
        _ => anything_else(tree, tkz, tok),
    }
}
