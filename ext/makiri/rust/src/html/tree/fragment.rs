//! The fragment parsing algorithm: Lexbor's `lxb_html_parse_fragment_chunk_begin`
//! / `_process` / `_end` with no owner document, under the parse guard, into
//! an arena document of the fragment's own.

use super::guard::{ContextOptions, DepthLimit, Guard, GuardStop};
use super::{Mode, TreeBuilder, TreeStatus};
use crate::html::dom::{Document, DomOom, NodeId};
use crate::html::res::names as res;
use crate::html::tokenizer::{Status, Tokenizer};
use crate::utf8_input::sanitize;

/// Why a fragment parse produced no fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentFailure {
    /// Out of memory repairing the input's UTF-8.
    Decode,
    /// The parse failed (out of memory, or the tree builder stopped).
    Parse,
    /// The guard refused it.
    Guard(GuardStop),
}

/// A parsed fragment: its own document, and the node whose children are the
/// fragment (Lexbor's `parser->root`, a synthetic `<html>`).
pub struct ParsedFragment {
    pub doc: Document,
    pub root: NodeId,
}

/// Parse `input` as a fragment in the context of an element with tag id
/// `tag` in namespace `ns` (Lexbor's static ids; `ns` 0 for none), refusing a
/// tree deeper than `limit`; `options` is what the context means to the
/// option guard.
pub fn parse_fragment(
    input: &[u8],
    known_valid: bool,
    tag: usize,
    ns: usize,
    options: ContextOptions,
    limit: DepthLimit,
) -> Result<ParsedFragment, FragmentFailure> {
    let src = sanitize(input, known_valid).ok_or(FragmentFailure::Decode)?;
    let bytes = src.as_slice();
    let oom = |DomOom| FragmentFailure::Parse;

    /* `lxb_html_document_interface_create(NULL)`: no quirks, no scripting. */
    let mut doc = Document::new(0).map_err(oom)?;
    doc.set_names(NodeId::DOCUMENT, res::TAG__DOCUMENT, res::NS_HTML as u32, 0);
    let ns32 = u32::try_from(ns).unwrap_or(0);
    let root = doc
        .create_element(res::TAG_HTML, res::NS_HTML as u32, 0, None)
        .map_err(oom)?;
    doc.append_child(NodeId::DOCUMENT, root);
    let context = doc.create_element(tag, ns32, 0, None).map_err(oom)?;

    let mut tree = TreeBuilder::new(&mut doc);
    tree.guard = Guard::for_fragment(limit, options);
    /* Nothing reads a fragment's parse errors. */
    tree.record_errors = false;
    tree.open_push(root).map_err(|_| FragmentFailure::Parse)?;
    if tag == res::TAG_TEMPLATE && ns == res::NS_HTML {
        tree.template_modes_push(Mode::InTemplate)
            .map_err(|_| FragmentFailure::Parse)?;
    }
    tree.fragment = Some(context);
    tree.reset_insertion_mode_appropriately();
    if tag == res::TAG_FORM && ns == res::NS_HTML {
        let form = tree
            .create_element_node(res::TAG_FORM, res::NS_HTML)
            .map_err(|_| FragmentFailure::Parse)?;
        tree.form = Some(form);
    }

    let status = {
        let current_ns = tree.current_namespace();
        let mut tkz = Tokenizer::new(bytes, &mut tree);
        /* `lxb_html_parser_init` */
        tkz.keep_duplicate = true;
        tkz.record_errors = false;
        tkz.set_state_by_tag(false, tag, ns);
        tkz.tmp_tag_id = res::TAG__UNDEF;
        tkz.current_ns = current_ns;
        match tkz.chunk() {
            Status::Ok => tkz.end(),
            st => st,
        }
    };
    if let Some(stop) = tree.guard.stopped {
        return Err(FragmentFailure::Guard(stop));
    }
    if status != Status::Ok || tree.status != TreeStatus::Ok {
        return Err(FragmentFailure::Parse);
    }
    drop(tree);
    Ok(ParsedFragment { doc, root })
}
