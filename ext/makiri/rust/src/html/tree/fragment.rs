//! The fragment parsing algorithm: Lexbor's `lxb_html_parse_fragment_chunk_begin`
//! / `_process` / `_end` with no owner document, under the parse guard, into
//! an arena document of the fragment's own.

use super::compat::{self, Fix};
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

/// What the fragment parsing algorithm reads from the context element and
/// its document besides the tag and the namespace. Lexbor read none of it;
/// the default is what it assumed, so a parse given the default is Lexbor's
/// however [`compat::fixed`] answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextFacts {
    /// The context document's quirks mode (0 no-quirks, 1 quirks, 2
    /// limited-quirks), which the fragment's document takes (P17).
    pub compat_mode: u8,
}

/// Parse `input` as a fragment in the context of an element with tag id
/// `tag` in namespace `ns` (Lexbor's static ids; `ns` 0 for none), refusing a
/// tree deeper than `limit`; `options` is what the context means to the
/// option guard, `facts` the rest of what the parse reads from it.
pub fn parse_fragment(
    input: &[u8],
    known_valid: bool,
    tag: usize,
    ns: usize,
    options: ContextOptions,
    facts: ContextFacts,
    limit: DepthLimit,
) -> Result<ParsedFragment, FragmentFailure> {
    let src = sanitize(input, known_valid).ok_or(FragmentFailure::Decode)?;
    let bytes = src.as_slice();
    let oom = |DomOom| FragmentFailure::Parse;

    /* `lxb_html_document_interface_create(NULL)`: no quirks, no scripting.
     * The Standard's document is in the context document's quirks mode. */
    let compat_mode = if compat::fixed(Fix::P17) {
        facts.compat_mode
    } else {
        0
    };
    let mut doc = Document::new(compat_mode).map_err(oom)?;
    doc.set_names(NodeId::DOCUMENT, res::TAG__DOCUMENT, res::NS_HTML as u32, 0);
    let ns32 = u32::try_from(ns).unwrap_or(0);
    let root = doc
        .create_element(res::TAG_HTML, res::NS_HTML as u32, 0, None)
        .map_err(oom)?;
    doc.append_child(NodeId::DOCUMENT, root);
    let context = doc.create_element(tag, ns32, 0, None).map_err(oom)?;

    let mut tree = TreeBuilder::new(&mut doc);
    tree.guard = Guard::for_fragment(limit, options);
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
    /* "The end" pops whatever is still open, with its pop steps, for a
     * fragment as for a document; Lexbor's fragment end did not. */
    if compat::fixed(Fix::P16) {
        tree.open_pop_all();
        if tree.status != TreeStatus::Ok {
            return Err(FragmentFailure::Parse);
        }
    }
    drop(tree);
    Ok(ParsedFragment { doc, root })
}
