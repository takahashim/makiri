//! The "initial" insertion mode (`insertion_mode/initial.c`).

use crate::html::dom::{CompatMode, NodeId};
use crate::html::res::names as res;
use crate::html::tokenizer::{flags, Tokenizer};
use crate::html::tree::error::TreeErrorId as E;
use crate::html::tree::{Mode, OrAbort, Tok, TreeBuilder};

/// "The public identifier is set to".
const PUBLIC_IS: [&[u8]; 3] = [
    b"-//W3O//DTD W3 HTML Strict 3.0//EN//",
    b"-/W3C/DTD HTML 4.0 Transitional/EN",
    b"HTML",
];

/// "The system identifier is set to".
const SYSTEM_IS: [&[u8]; 1] = [b"http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"];

/// "The public identifier starts with".
const PUBLIC_START: [&[u8]; 55] = [
    b"+//Silmaril//dtd html Pro v0r11 19970101//",
    b"-//AS//DTD HTML 3.0 asWedit + extensions//",
    b"-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
    b"-//IETF//DTD HTML 2.0 Level 1//",
    b"-//IETF//DTD HTML 2.0 Level 2//",
    b"-//IETF//DTD HTML 2.0 Strict Level 1//",
    b"-//IETF//DTD HTML 2.0 Strict Level 2//",
    b"-//IETF//DTD HTML 2.0 Strict//",
    b"-//IETF//DTD HTML 2.0//",
    b"-//IETF//DTD HTML 2.1E//",
    b"-//IETF//DTD HTML 3.0//",
    b"-//IETF//DTD HTML 3.2 Final//",
    b"-//IETF//DTD HTML 3.2//",
    b"-//IETF//DTD HTML 3//",
    b"-//IETF//DTD HTML Level 0//",
    b"-//IETF//DTD HTML Level 1//",
    b"-//IETF//DTD HTML Level 2//",
    b"-//IETF//DTD HTML Level 3//",
    b"-//IETF//DTD HTML Strict Level 0//",
    b"-//IETF//DTD HTML Strict Level 1//",
    b"-//IETF//DTD HTML Strict Level 2//",
    b"-//IETF//DTD HTML Strict Level 3//",
    b"-//IETF//DTD HTML Strict//",
    b"-//IETF//DTD HTML//",
    b"-//Metrius//DTD Metrius Presentational//",
    b"-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
    b"-//Microsoft//DTD Internet Explorer 2.0 HTML//",
    b"-//Microsoft//DTD Internet Explorer 2.0 Tables//",
    b"-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
    b"-//Microsoft//DTD Internet Explorer 3.0 HTML//",
    b"-//Microsoft//DTD Internet Explorer 3.0 Tables//",
    b"-//Netscape Comm. Corp.//DTD HTML//",
    b"-//Netscape Comm. Corp.//DTD Strict HTML//",
    b"-//O'Reilly and Associates//DTD HTML 2.0//",
    b"-//O'Reilly and Associates//DTD HTML Extended 1.0//",
    b"-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
    b"-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
    b"-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
    b"-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
    b"-//Spyglass//DTD HTML 2.0 Extended//",
    b"-//Sun Microsystems Corp.//DTD HotJava HTML//",
    b"-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
    b"-//W3C//DTD HTML 3 1995-03-24//",
    b"-//W3C//DTD HTML 3.2 Draft//",
    b"-//W3C//DTD HTML 3.2 Final//",
    b"-//W3C//DTD HTML 3.2//",
    b"-//W3C//DTD HTML 3.2S Draft//",
    b"-//W3C//DTD HTML 4.0 Frameset//",
    b"-//W3C//DTD HTML 4.0 Transitional//",
    b"-//W3C//DTD HTML Experimental 19960712//",
    b"-//W3C//DTD HTML Experimental 970421//",
    b"-//W3C//DTD W3 HTML//",
    b"-//W3O//DTD W3 HTML 3.0//",
    b"-//WebTechs//DTD Mozilla HTML 2.0//",
    b"-//WebTechs//DTD Mozilla HTML//",
];

/// "The system identifier is missing and the public identifier starts with".
const SYS_PUB_START: [&[u8]; 2] = [
    b"-//W3C//DTD HTML 4.01 Frameset//",
    b"-//W3C//DTD HTML 4.01 Transitional//",
];

/// Limited quirks: "the public identifier starts with".
const LIM_PUB_START: [&[u8]; 2] = [
    b"-//W3C//DTD XHTML 1.0 Frameset//",
    b"-//W3C//DTD XHTML 1.0 Transitional//",
];

fn starts_with_ci(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// `lxb_html_tree_insertion_mode_initial`.
pub fn initial(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &mut Tok) -> bool {
    match tok.tag_id {
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
        res::TAG__EM_DOCTYPE => {
            tree.mode = Mode::BeforeHtml;
            return initial_doctype(tree, tkz, tok);
        }
        _ => {
            if tok.tag_id == res::TAG__TEXT {
                tok.skip_ws_begin(tkz);
                if tok.text_start == tok.text_end {
                    return true;
                }
            }
            /* No iframe srcdoc document is ever parsed here. */
            tree.parse_error(tok, E::Untoininmo);
            tree.doc.set_compat_mode(CompatMode::Quirks);
            tree.mode = Mode::BeforeHtml;
            return false;
        }
    }
    true
}

/// `lxb_html_tree_insertion_mode_initial_doctype`.
fn initial_doctype(tree: &mut TreeBuilder<'_>, tkz: &mut Tokenizer<'_, '_>, tok: &Tok) -> bool {
    let Some((node, name_id, _, _)) = tree.create_document_type_from_token(tkz).or_abort(tree)
    else {
        return true;
    };
    let (public, system) = match tree.doc.doctype(node) {
        Some((_, p, s)) => (p.len(), s),
        None => (0, &b""[..]),
    };
    let is_html = name_id == res::ATTR_HTML;
    if !is_html || public != 0 || (system.len() == 19 && system != b"about:legacy-compat") {
        tree.parse_error(tok, E::Badotoininmo);
    }
    doctype_check(tree, node, tok, is_html);
    tree.doc.append_child(NodeId::DOCUMENT, node);
    true
}

/// `lxb_html_tree_insertion_mode_initial_doctype_ckeck`.
fn doctype_check(tree: &mut TreeBuilder<'_>, node: NodeId, tok: &Tok, is_html: bool) {
    let Some((_, public, system)) = tree.doc.doctype(node) else {
        return;
    };
    let quirks = tok.flags & flags::FORCE_QUIRKS != 0
        || !is_html
        || (!public.is_empty() && check_public(public))
        || (!system.is_empty() && check_system(system))
        || (!public.is_empty() && system.is_empty() && check_pubsys(public));
    if quirks {
        tree.doc.set_compat_mode(CompatMode::Quirks);
        return;
    }
    if !public.is_empty() && check_limq(public, system) {
        tree.doc.set_compat_mode(CompatMode::LimitedQuirks);
    }
}

/// `..._doctype_ckeck_public`.
fn check_public(public: &[u8]) -> bool {
    PUBLIC_IS.iter().any(|s| public.eq_ignore_ascii_case(s))
        || PUBLIC_START.iter().any(|s| starts_with_ci(public, s))
}

/// `..._doctype_ckeck_system`.
fn check_system(system: &[u8]) -> bool {
    SYSTEM_IS.iter().any(|s| system.eq_ignore_ascii_case(s))
}

/// `..._doctype_ckeck_pubsys`.
fn check_pubsys(public: &[u8]) -> bool {
    SYS_PUB_START.iter().any(|s| starts_with_ci(public, s))
}

/// `..._doctype_check_limq`.
fn check_limq(public: &[u8], system: &[u8]) -> bool {
    (!system.is_empty() && check_pubsys(public))
        || LIM_PUB_START.iter().any(|s| starts_with_ci(public, s))
}
