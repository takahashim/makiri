//! The ported stylesheet parser (`crate::selectors::parse_stylesheet`)
//! against Lexbor's (`lxb_css_stylesheet_parse`, behind `contains_guard` as
//! Makiri ran it), as data: both parse the same text and their rule trees -
//! every rule's kind, offsets and selectors, every at-rule's kind and custom
//! name, every declaration's name, value offsets and `!important`, nested
//! blocks and the rules a style rule keeps as children included - are dumped
//! and compared (`docs/rewrite_parser.ja.md`, the stylesheet parser).
//! `lexbor::selector_port_tests` and the `stylesheet_diff` fuzz target use it.
//!
//! A declaration's name is compared as the reader writes it (lower-cased
//! unless custom): Lexbor writes a known property's canonical name, which is
//! the lower-cased ident.

#![allow(unsafe_code)]
#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use core::fmt::Write as _;

use crate::lexbor::abi as lxb;
use crate::lexbor::abi::consts as k;
use crate::lexbor::abi::{lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init};
use crate::lexbor::chunks::{chunk_cb, Chunks};
use crate::lexbor::css_engine::{lexbor_str, Owned};
use crate::lexbor::css_parser::list_from_raw;
use crate::lexbor::selector_diff::{dump_lexbor, dump_port};
use crate::selectors::rules::{AtKind, Item, ItemId};
use crate::selectors::{Sheet, Status};

/// How deep the dumps descend: both stop at the same depth and write a
/// marker there, so a deeper sheet is compared down to it rather than
/// overflowing the dumpers' recursion.
const MAX_DEPTH: usize = 128;

fn name_as_written(name: &[u8]) -> String {
    let mut n = String::from_utf8_lossy(name).into_owned();
    if !n.starts_with("--") {
        n.make_ascii_lowercase();
    }
    n
}

/* ---- Lexbor ---- */

unsafe fn property_name(decl: *mut lxb::lxb_css_rule_declaration_t) -> Vec<u8> {
    let mut s = Chunks::new(Vec::new());
    let st = lxb::lxb_css_property_serialize_name(
        (*decl).u.user,
        (*decl).type_,
        Some(chunk_cb::<Vec<u8>>),
        s.ctx(),
    );
    assert_eq!(st, 0, "serialize_name failed");
    core::mem::take(&mut s.sink)
}

unsafe fn lexbor_list(first: *mut lxb::lxb_css_rule_t, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH {
        out.push_str("...\n");
        return;
    }
    let pad = " ".repeat(depth);
    let mut r = first;
    while !r.is_null() {
        match (*r).type_ as usize {
            k::CSS_RULE_STYLE => {
                let st = r as *mut lxb::lxb_css_rule_style_t;
                let _ = write!(
                    out,
                    "{pad}style {}..{} ",
                    (*st).prelude_begin,
                    (*st).prelude_end
                );
                dump_lexbor(list_from_raw((*st).selector), out);
                out.push('\n');
                lexbor_decls((*st).declarations, depth + 1, out);
                lexbor_child((*st).child, depth + 1, out);
            }
            k::CSS_RULE_BAD_STYLE => {
                let bad = r as *mut lxb::lxb_css_rule_bad_style_t;
                let _ = writeln!(
                    out,
                    "{pad}bad {}..{}",
                    (*bad).prelude_begin,
                    (*bad).prelude_end
                );
                lexbor_decls((*bad).declarations, depth + 1, out);
                lexbor_child((*bad).child, depth + 1, out);
            }
            k::CSS_RULE_AT_RULE => {
                let at = r as *mut lxb::lxb_css_rule_at_t;
                let null = core::ptr::null_mut;
                let (kind, block) = match (*at).type_ {
                    k::AT_RULE_MEDIA => (
                        "media".to_owned(),
                        (*at).u.media.as_ref().map_or(null(), |m| m.block),
                    ),
                    k::AT_RULE_FONT_FACE => (
                        "font-face".to_owned(),
                        (*at).u.font_face.as_ref().map_or(null(), |f| f.block),
                    ),
                    k::AT_RULE_NAMESPACE => ("namespace".to_owned(), null()),
                    k::AT_RULE_CUSTOM => {
                        let cu = (*at).u.custom.as_ref();
                        let name = cu.and_then(|c| lexbor_str(&c.name)).unwrap_or(b"");
                        (
                            format!("custom:{}", String::from_utf8_lossy(name)),
                            cu.map_or(null(), |c| c.block),
                        )
                    }
                    k::AT_RULE_UNDEF => (
                        "undef".to_owned(),
                        (*at).u.undef.as_ref().map_or(null(), |u| u.block),
                    ),
                    t => (format!("type{t}"), null()),
                };
                let _ = writeln!(
                    out,
                    "{pad}at {kind} {}..{}",
                    (*at).prelude_begin,
                    (*at).prelude_end
                );
                lexbor_child(block, depth + 1, out);
            }
            t if t == lxb::lxb_css_rule_type_t_LXB_CSS_RULE_DECLARATION_LIST as usize => {
                lexbor_decls(r as *mut lxb::lxb_css_rule_declaration_list_t, depth, out);
            }
            t => {
                let _ = writeln!(out, "{pad}rule{t}");
            }
        }
        r = (*r).next;
    }
}

unsafe fn lexbor_child(list: *mut lxb::lxb_css_rule_list_t, depth: usize, out: &mut String) {
    if let Some(l) = list.as_ref() {
        let _ = writeln!(out, "{}list", " ".repeat(depth));
        lexbor_list(l.first, depth + 1, out);
    }
}

unsafe fn lexbor_decls(
    list: *mut lxb::lxb_css_rule_declaration_list_t,
    depth: usize,
    out: &mut String,
) {
    let Some(l) = list.as_ref() else {
        return;
    };
    let pad = " ".repeat(depth);
    let _ = writeln!(out, "{pad}decls");
    let mut r = l.first;
    while !r.is_null() {
        if (*r).type_ as usize == k::CSS_RULE_DECLARATION {
            let d = r as *mut lxb::lxb_css_rule_declaration_t;
            let off = (*d).offset;
            let _ = writeln!(
                out,
                "{pad} decl {} {}..{} {}",
                name_as_written(&property_name(d)),
                off.value_begin,
                off.value_end,
                (*d).important
            );
        }
        r = (*r).next;
    }
}

/// What Lexbor made of a stylesheet, or its status when the parse failed.
pub fn lexbor_dump(css: &[u8]) -> Result<String, u32> {
    let parsed = crate::lexbor::contains_guard::neutralized(css).expect("oom");
    // SAFETY: every pointer is created by the Lexbor calls below and owned
    // by an `Owned` from then on; the text is a live slice the parser only
    // reads, and the tree is read while the stylesheet lives.
    unsafe {
        let sheet = Owned::new(
            lxb::lxb_css_stylesheet_create(core::ptr::null_mut()),
            lxb::lxb_css_stylesheet_destroy,
        )
        .expect("stylesheet");
        let parser = Owned::new(lxb_css_parser_create(), lxb_css_parser_destroy).expect("parser");
        assert_eq!(
            lxb_css_parser_init(parser.as_ptr(), core::ptr::null_mut()),
            0
        );
        let source = parsed.as_deref().unwrap_or(css);
        let st = lxb::lxb_css_stylesheet_parse(
            sheet.as_ptr(),
            parser.as_ptr(),
            source.as_ptr(),
            source.len(),
        );
        if st != k::STATUS_OK {
            return Err(st);
        }
        let mut out = String::new();
        let root = (*sheet.as_ptr()).root;
        if let Some(list) = (root as *mut lxb::lxb_css_rule_list_t).as_ref() {
            lexbor_list(list.first, 0, &mut out);
        }
        drop(parser);
        Ok(out)
    }
}

/* ---- the port ---- */

fn port_list(sheet: &Sheet, list: Option<ItemId>, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH {
        out.push_str("...\n");
        return;
    }
    let pad = " ".repeat(depth);
    let name = |span: crate::selectors::token::Span| {
        let s = span.start as usize;
        sheet.ast.strings[s..s + span.len as usize].to_vec()
    };
    for (id, item) in sheet.rules.children(list) {
        match *item {
            Item::Style {
                selector,
                prelude_begin,
                prelude_end,
                declarations,
                child,
            } => {
                let _ = write!(out, "{pad}style {prelude_begin}..{prelude_end} ");
                dump_port(sheet.ast.lists_from(selector), out);
                out.push('\n');
                port_decls(sheet, declarations, depth + 1, out);
                port_child(sheet, child, depth + 1, out);
            }
            Item::BadStyle {
                prelude_begin,
                prelude_end,
                declarations,
                child,
            } => {
                let _ = writeln!(out, "{pad}bad {prelude_begin}..{prelude_end}");
                port_decls(sheet, declarations, depth + 1, out);
                port_child(sheet, child, depth + 1, out);
            }
            Item::At {
                kind,
                name: n,
                prelude_begin,
                prelude_end,
                block,
            } => {
                let kind = match kind {
                    AtKind::Media => "media".to_owned(),
                    AtKind::FontFace => "font-face".to_owned(),
                    AtKind::Namespace => "namespace".to_owned(),
                    AtKind::Custom => format!("custom:{}", String::from_utf8_lossy(&name(n))),
                    AtKind::Undef => "undef".to_owned(),
                };
                let _ = writeln!(out, "{pad}at {kind} {prelude_begin}..{prelude_end}");
                port_child(sheet, block, depth + 1, out);
            }
            Item::DeclarationList { .. } => port_decls(sheet, Some(id), depth, out),
            _ => {
                let _ = writeln!(out, "{pad}rule?");
            }
        }
    }
}

fn port_child(sheet: &Sheet, list: Option<ItemId>, depth: usize, out: &mut String) {
    if list.is_some() {
        let _ = writeln!(out, "{}list", " ".repeat(depth));
        port_list(sheet, list, depth + 1, out);
    }
}

fn port_decls(sheet: &Sheet, list: Option<ItemId>, depth: usize, out: &mut String) {
    if list.is_none() {
        return;
    }
    let pad = " ".repeat(depth);
    let _ = writeln!(out, "{pad}decls");
    for (_, item) in sheet.rules.children(list) {
        if let Item::Declaration {
            name,
            value_begin,
            value_end,
            important,
        } = *item
        {
            let s = name.start as usize;
            let n = &sheet.ast.strings[s..s + name.len as usize];
            let _ = writeln!(
                out,
                "{pad} decl {} {value_begin}..{value_end} {important}",
                name_as_written(n)
            );
        }
    }
}

/// What the port made of a stylesheet, or its status when the parse failed.
pub fn port_dump(css: &[u8]) -> Result<String, Status> {
    let sheet = crate::selectors::parse_stylesheet(css)?;
    let mut out = String::new();
    port_list(&sheet, Some(sheet.root), 0, &mut out);
    Ok(out)
}

/// Panic unless both parsers make the same of `css`, which must be what
/// Makiri parses: UTF-8 with no NUL.
pub fn check(css: &[u8]) {
    let lexbor = lexbor_dump(css);
    let port = port_dump(css);
    match (&lexbor, &port) {
        (Ok(a), Ok(b)) => assert_eq!(
            a,
            b,
            "the parsers differ on {:?}",
            String::from_utf8_lossy(css)
        ),
        (Err(_), Err(Status::ErrorUnexpectedData)) => {}
        _ => panic!(
            "the parsers differ on {:?}: {lexbor:?} / {port:?}",
            String::from_utf8_lossy(css)
        ),
    }
}
