//! The fragment parsing algorithm: Lexbor's `lxb_html_parse_fragment_chunk_begin`
//! / `_process` / `_end` with no owner document, under the parse guard, into
//! an arena document of the fragment's own.

use super::guard::{ContextOptions, DepthLimit, Guard, GuardStop};
use super::parse::drive;
use super::{Mode, TreeBuilder, TreeStatus};
use crate::compat::{self, Fix};
use crate::html::dom::{Document, DomOom, NodeId};
use crate::html::res::names as res;
use crate::html::tokenizer::Status;
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
    /// The context is a MathML `annotation-xml` whose `encoding` makes it
    /// an HTML integration point (P18): the attributes the Standard's
    /// context start tag token carries, as far as the parse reads them.
    pub html_integration_point: bool,
    /// The context or one of its ancestors is an HTML `form`, which the form
    /// element pointer is set to (P20).
    pub in_form: bool,
    /// The context is an element whose namespace the parser is given as
    /// none (it has none, or one the parser does not know), which is as
    /// foreign as any namespace but HTML: a `<![CDATA[` opens a CDATA
    /// section in it (P19).
    pub foreign_namespace: bool,
}

/// The context element of a fragment parse, as far as the parse reads it:
/// its tag and namespace (Lexbor's static ids; `ns` 0 for none), what it
/// means to the option guard, and the rest ([`ContextFacts`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextElement {
    pub tag: usize,
    pub ns: usize,
    pub options: ContextOptions,
    pub facts: ContextFacts,
}

impl ContextElement {
    /// A context known by its name alone - a fragment bound to a document,
    /// the WPT fragment tests: it leads an option to a select only by being
    /// one, and its facts are Lexbor's defaults.
    pub fn named(tag: usize, ns: usize) -> ContextElement {
        let select = tag == res::TAG_SELECT && ns == res::NS_HTML;
        ContextElement {
            tag,
            ns,
            options: ContextOptions {
                fresh: select,
                after_optgroup: select,
            },
            facts: ContextFacts::default(),
        }
    }
}

/// Parse `input` as a fragment in `context`, refusing a tree deeper than
/// `limit`.
pub fn parse_fragment(
    input: &[u8],
    known_valid: bool,
    context: &ContextElement,
    limit: DepthLimit,
) -> Result<ParsedFragment, FragmentFailure> {
    let ContextElement {
        tag,
        ns,
        options,
        facts,
    } = *context;
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
    let ns32 = u32::try_from(ns).unwrap_or(0);
    let root = doc
        .create_element(res::TAG_HTML, res::NS_HTML as u32, 0, None)
        .map_err(oom)?;
    doc.append_child(NodeId::DOCUMENT, root);
    let context = doc.create_element(tag, ns32, 0, None).map_err(oom)?;

    let mut tree = TreeBuilder::for_fragment(
        &mut doc,
        root,
        context,
        &facts,
        Guard::for_fragment(limit, options),
    )
    .map_err(|_| FragmentFailure::Parse)?;

    let current_ns = tree.current_namespace();
    let foreign = facts.foreign_namespace && compat::fixed(Fix::P19);
    let status = drive(&mut tree, bytes, foreign, |tkz| {
        tkz.enter_fragment_context(false, tag, ns);
        tkz.set_current_namespace(current_ns);
    });
    if let Some(stop) = tree.guard_stopped() {
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

impl<'d> TreeBuilder<'d> {
    /// A fragment parse into `doc`, held to `guard`, set up as Lexbor's
    /// `lxb_html_parse_fragment_chunk_begin` does: `root` the only open
    /// element, `context` the context element, a template's mode, the
    /// insertion mode reset, and the form element pointer.
    fn for_fragment(
        doc: &'d mut Document,
        root: NodeId,
        context: NodeId,
        facts: &ContextFacts,
        guard: Guard,
    ) -> Result<TreeBuilder<'d>, TreeStatus> {
        let mut tree = TreeBuilder::new(doc, guard);
        let (tag, ns) = (tree.tag(context), tree.ns(context));
        tree.open_push(root)?;
        if tag == res::TAG_TEMPLATE && ns == res::NS_HTML {
            tree.template_modes_push(Mode::InTemplate)?;
        }
        tree.fragment = Some(context);
        tree.context_integration_point = facts.html_integration_point && compat::fixed(Fix::P18);
        tree.reset_insertion_mode_appropriately();
        let own_form = tag == res::TAG_FORM && ns == res::NS_HTML;
        if own_form || (facts.in_form && compat::fixed(Fix::P20)) {
            tree.form = Some(tree.create_element_node(res::TAG_FORM, res::NS_HTML)?);
        }
        Ok(tree)
    }
}
