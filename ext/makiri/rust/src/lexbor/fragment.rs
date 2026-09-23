//! The HTML fragment pipeline.
//!
//! Parsing a fragment, importing its children into a document, and the
//! `<template>`-content fixup that `lxb_dom_document_import_node` omits.
//!
//! # Why this is not with the Document
//!
//! None of this is about the Document WRAPPER - it is a service the wrapper
//! happens to use and the mutators and `import_node` use directly, and keeping
//! it here means the module that owns `Makiri::HTML::Document` is about that
//! class rather than about three unrelated things.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::falloc::VecPush;

use crate::lexbor::abi::{LxbDoc, LxbNode};

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::lexbor::adapter::html::{BuildingNode, HtmlDoc, HtmlNode, RawDoc, RawNode};
use crate::lexbor::adapter::utf8_input::sanitize;

/* The two fragment parsers, both generated. Everything this file does to the
 * DOM itself goes through `lexbor::adapter::html` - these are the parser, not
 * the DOM. */
use crate::lexbor::abi::{lxb_html_parse_fragment, lxb_html_parse_fragment_by_tag_id};

/* The HTML parser's lifecycle, from the generated bindings. Declared here first
 * over an opaque parser, which was fine until the source-location port needed
 * the tokenizer inside it and build.rs started generating them - two Rust types
 * for one symbol again. */
use crate::lexbor::abi::HtmlParser;

/// `lxb_dom_document_import_node` deep-clones the normal child chain but NOT a
/// `<template>`'s separate content fragment, so an imported template comes out
/// empty. Walk source and clone in lockstep (deep import preserves child order
/// 1:1) and import each template's content into the clone's.
///
/// Iterative, with an explicit worklist: an adversarially deep fragment must not
/// be able to overflow the stack. Best-effort on allocation failure, as the C
/// was - a template whose content could not be copied is left empty rather than
/// the whole import failing.
///
/// `template_content` answers in one what this used to ask in three: it is
/// `None` for a node that is not an HTML `<template>` AND for one Lexbor gave no
/// contents fragment. Those are the same case here, because the only thing this
/// walk does is copy one existing contents fragment into another - which is why
/// `cross_import`'s `h2x_children_of`, where an empty template and a
/// non-template mean DIFFERENT children, keeps a test of its own.
fn fixup_template_content(
    doc: HtmlDoc<'_>,
    root_src: HtmlNode<'_>,
    root_clone: BuildingNode<'_>,
) -> Result<(), ()> {
    let mut stack: Vec<(HtmlNode<'_>, BuildingNode<'_>)> = Vec::new();
    stack.falloc_push((root_src, root_clone))?;

    while let Some((src_root, clone_root)) = stack.pop() {
        let (mut sn, mut cn) = (Some(src_root), Some(clone_root));
        while let (Some(s), Some(c)) = (sn, cn) {
            /* Nested rather than a tuple: the clone-side test is only worth
             * paying for once the source side has said this is a template, and
             * every node of every deep import passes through here. */
            if let Some((sc, cc)) = s
                .template_content()
                .and_then(|sc| Some((sc, c.template_content()?)))
            {
                let mut x = sc.first_child();
                while let Some(child) = x {
                    let imp = doc.import_node(child, true);
                    let Some(imp) = imp else {
                        // Lexbor could not copy a content child. Giving up
                        // here leaves the clone's template SHORT, which is
                        // the truncated answer the contract forbids.
                        return Err(());
                    };
                    cc.insert_child(imp);
                    x = child.next();
                }
                stack.falloc_push((sc, cc))?;
            }
            sn = s.preorder_next(src_root);
            cn = c.preorder_next(clone_root);
        }
    }
    Ok(())
}

/// Why a fragment parse produced no fragment. The bridge words it for Ruby.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentError {
    /// Lexbor could not create or initialise a parser.
    Parser,
    /// Out of memory repairing the input's UTF-8.
    Decode,
    /// The parse itself returned no fragment.
    Parse,
}

impl FragmentError {
    pub fn message(self) -> &'static str {
        match self {
            FragmentError::Parser => "failed to create HTML parser",
            FragmentError::Decode => "out of memory decoding fragment HTML",
            FragmentError::Parse => "failed to parse HTML fragment",
        }
    }
}

/// Deep-import each child of `root` into `doc`, appending each to `into`.
///
/// `false` when a child could not be copied whole. It REPORTS rather than
/// raising, and that still matters now the C has gone: every caller owns the
/// transient document the fragment was parsed into
/// (`lexbor::abi::TransientDoc`), and a raise from here would longjmp past its
/// `Drop` - one leaked Lexbor document per failure. The caller raises once its
/// own cleanup has run, with the message that suits it.
unsafe fn import_fragment_children(doc: RawDoc, root: RawNode, into: RawNode) -> bool {
    let Some(into) = BuildingNode::from_raw(into.as_ptr() as *mut LxbNode) else {
        return false;
    };
    let mut f = (*(root.as_ptr() as *mut LxbNode)).first_child;
    while !f.is_null() {
        let next = (*f).next; /* import does not unlink f, but be safe */
        match import_raw(doc, f, true) {
            Some(imp) => {
                /* A node import just made in `doc`, not yet in any tree. */
                if let Some(imp) = BuildingNode::from_raw(imp.cast()) {
                    into.insert_child(imp);
                }
            }
            None => return false,
        }
        f = next;
    }
    true
}

/// A fragment parsed in a context - an element, or a tag and namespace. With an
/// element context it lives in a TRANSIENT document Lexbor builds for it, which
/// destroying the parser does not free, so this owns it and frees it on drop,
/// whatever happens in between (see `parse` for why only that context).
///
/// Parsing and importing are separate steps so a caller can parse FIRST and
/// change its tree only once the input has turned out to be usable:
/// `inner_html=` used to empty the element and then fail to parse, which
/// raised with the old children already gone.
pub struct TransientFragment {
    root: RawNode,
    _doc: Option<crate::lexbor::abi::TransientDoc>,
}

impl TransientFragment {
    /// # Safety
    /// `context`'s element or document must be live; `input` is only read.
    pub unsafe fn parse(
        input: &[u8],
        known_valid: bool,
        context: &FragmentContext,
    ) -> Result<TransientFragment, FragmentError> {
        let root = run_fragment_parser(input, known_valid, context)?;
        /* Only the element context gets a document of its own to free.
         * `lxb_html_parse_fragment_chunk_begin` builds the fragment in
         * `lxb_html_document_interface_create(owner)`: the element parser passes
         * its fresh parser's tree document - NULL - so the result is standalone
         * and must be destroyed here; the by-tag parser is handed the TARGET
         * document, so its result is made inside that document's memory and
         * destroying it would free the target's. */
        let _doc = match context {
            FragmentContext::Element(_) => {
                crate::lexbor::abi::TransientDoc::of(root.as_ptr() as *mut LxbNode)
            }
            FragmentContext::Tag { .. } => None,
        };
        Ok(TransientFragment { root, _doc })
    }

    /// Import every child into `doc`, as the children of `into`; `false` if
    /// one failed, with the ones before it already there.
    ///
    /// Always into a detached DOCUMENT_FRAGMENT, never into a live tree: a
    /// failure part-way is then invisible, and the caller places the whole
    /// fragment afterwards (see `bridge::fragment::stage_fragment_in`).
    ///
    /// # Safety
    /// `doc` must be live, and `into` a detached fragment of `doc` that nothing
    /// else refers to.
    pub unsafe fn import_into(self, doc: RawDoc, into: RawNode) -> bool {
        import_fragment_children(doc, self.root, into)
    }
}

/// Which Lexbor fragment parser to run, and the context it needs.
///
/// Both implement the same WHATWG algorithm - tokenizer state for
/// rawtext/rcdata, foreign-content adjustment, the form pointer - and differ
/// only in how the context arrives. The context used to be a `*mut c_void`
/// beside a function pointer, cast back by whichever callback was chosen; it
/// travels with the choice now.
pub enum FragmentContext {
    /// The context element itself, which `inner_html=` and `outer_html=` have.
    Element(RawNode),
    /// A named context: a tag id and namespace, for `Document#fragment` and
    /// `DocumentFragment.parse`, where no such element exists yet.
    Tag { doc: RawDoc, tag: usize, ns: usize },
}

impl FragmentContext {
    /// # Safety
    /// `parser` must be live and initialised, and the source bytes must stay
    /// put for the call. The context - element or document - must be live.
    unsafe fn parse(&self, parser: &HtmlParser, src: *const u8, len: usize) -> *mut LxbNode {
        match *self {
            /* Lexbor types this one to its element interface; the handle we
             * hold is a node, which is what that interface begins with. */
            FragmentContext::Element(el) => {
                lxb_html_parse_fragment(parser.as_ptr(), el.as_ptr() as *mut _, src, len)
            }
            /* A document handle is untyped; this entry takes the HTML document
             * it is. */
            FragmentContext::Tag { doc, tag, ns } => lxb_html_parse_fragment_by_tag_id(
                parser.as_ptr(),
                doc.as_ptr().cast(),
                tag,
                ns,
                src,
                len,
            ),
        }
    }
}

/// Run a fragment parse with a fresh parser, or the error that stopped it.
///
/// The parser is destroyed on every path: the fragment tree belongs to its
/// document, not the parser, so it survives - the caller may still read
/// `root->owner_document` afterwards.
unsafe fn run_fragment_parser(
    input: &[u8],
    known_valid: bool,
    context: &FragmentContext,
) -> Result<RawNode, FragmentError> {
    let parser = HtmlParser::create().ok_or(FragmentError::Parser)?;
    let src = sanitize(input, known_valid).ok_or(FragmentError::Decode)?;
    let bytes = src.as_slice();
    let root = context.parse(&parser, bytes.as_ptr(), bytes.len());
    drop(src); /* the parse consumed it; the buffer goes on every path */
    drop(parser); /* the fragment belongs to its document, not to the parser */
    RawNode::from_ptr(root.cast()).ok_or(FragmentError::Parse)
}

/// Copy `src` into `doc`, `<template>` contents included, or `None` on failure.
///
/// The DOM `importNode` omits a template's separate content fragment, so every
/// copy in this extension is import-plus-fixup; this is that one operation.
/// Three callers wanted it with different deep flags, different error channels
/// and different messages, and each had grown its own copy of the four lines -
/// so the operation lives here and they keep only the parts that differ.
pub unsafe fn import_with_fixup(doc: RawDoc, src: RawNode, deep: bool) -> Option<RawNode> {
    import_raw(doc, src.as_ptr() as *mut LxbNode, deep).and_then(|p| RawNode::from_ptr(p.cast()))
}

/// The raw form of [`import_with_fixup`]: `src` is a Lexbor node pointer, which
/// only this module uses, for the children it walks itself.
unsafe fn import_raw(doc: RawDoc, src: *mut LxbNode, deep: bool) -> Option<*mut LxbNode> {
    /* SAFETY: a live document and the caller's live source node. The handles
     * do not outlive this call. */
    let (Some(hdoc), Some(hsrc)) = (
        HtmlDoc::from_raw(doc.as_ptr() as *mut LxbDoc),
        HtmlNode::from_raw(src),
    ) else {
        return None;
    };
    let himp = hdoc.import_node(hsrc, deep)?;
    let imp = himp.as_raw();
    if deep && fixup_template_content(hdoc, hsrc, himp).is_err() {
        // A copy whose <template> lost its contents is a wrong answer, not a
        // degraded one: `<template><i>x</i></template>` comes back as
        // `<template></template>` and nothing says so. The C was best-effort
        // here and the port carried that over; the OOM sweep called it, which
        // is what the sweep is for.
        return None;
    }
    if hsrc.owner_document() != hdoc {
        /* The copied offsets index the other document's source. */
        himp.clear_source_offsets();
    }
    Some(imp)
}

/* ------------------------------------------------------------------ */
/* the context helpers the Ruby-facing bridge drives                   */
/* ------------------------------------------------------------------ */

/// The tag id Lexbor knows `name` by, or `TAG_UNDEF` for an unknown name.
///
/// The Ruby-facing context resolution lives in [`crate::bridge::fragment`];
/// the lookup itself is [`HtmlDoc::tag_id`].
pub fn tag_id_by_name(doc: RawDoc, name: &[u8]) -> usize {
    // SAFETY: a live document handle, read for this call.
    unsafe { doc.as_doc() }.tag_id(name)
}
