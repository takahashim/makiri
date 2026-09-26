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
use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::lexbor::adapter::html::{
    BuildingNode, HtmlDoc, HtmlElement, HtmlNode, NsId, RawDoc, RawNode, TagId,
};
use crate::lexbor::adapter::AdapterOom;

/* The chunked fragment parser, generated. Everything this file does to the
 * DOM itself goes through `lexbor::adapter::html` - these are the parser, not
 * the DOM. */
use crate::lexbor::abi::{
    lxb_html_parse_fragment_chunk_begin, lxb_html_parse_fragment_chunk_end,
    lxb_html_parse_fragment_chunk_process, TransientDoc,
};
use crate::lexbor::adapter::tree_guard::{fragment_document, DepthLimit, GuardStop, TokenHook};

/* The HTML parser's lifecycle, from the generated bindings. Declared here first
 * over an opaque parser, which was fine until the source-location port needed
 * the tokenizer inside it and build.rs started generating them - two Rust types
 * for one symbol again. */
use crate::lexbor::abi::HtmlParser;
use crate::utf8_input::sanitize;

/// `lxb_dom_document_import_node` deep-clones the normal child chain but NOT a
/// `<template>`'s separate content fragment, so an imported template comes out
/// empty. Walk source and clone in lockstep (deep import preserves child order
/// 1:1) and import each template's content into the clone's.
///
/// The same lockstep walk gives each copied element the name its source
/// records as written, which Lexbor's copy also leaves out
/// (`BuildingNode::copy_written_name_from`).
///
/// Iterative, with an explicit worklist: an adversarially deep fragment must not
/// be able to overflow the stack. Fail-closed on allocation failure: a template
/// content that could not be copied refuses the whole import rather than
/// leaving a clone that is silently short.
///
/// `template_content` answers in one what this used to ask in three: it is
/// `None` for a node that is not an HTML `<template>` AND for one Lexbor gave no
/// contents fragment. Those are the same case here, because the only thing this
/// walk does is copy one existing contents fragment into another - which is why
/// `cross_import`'s `h2x_first_child`, where an empty template and a
/// non-template mean DIFFERENT children, keeps a test of its own.
fn fixup_template_content(
    doc: HtmlDoc<'_>,
    root_src: HtmlNode<'_>,
    root_clone: BuildingNode<'_>,
) -> Result<(), AdapterOom> {
    let mut stack: Vec<(HtmlNode<'_>, BuildingNode<'_>)> = Vec::new();
    /* The worklist's own allocation failing refuses the copy as surely as
     * Lexbor's does. */
    stack
        .falloc_push((root_src, root_clone))
        .map_err(|()| AdapterOom)?;

    while let Some((src_root, clone_root)) = stack.pop() {
        let (mut sn, mut cn) = (Some(src_root), Some(clone_root));
        while let (Some(s), Some(c)) = (sn, cn) {
            /* Nested rather than a tuple: the clone-side test is only worth
             * paying for once the source side has said this is a template, and
             * every node of every deep import passes through here. */
            c.copy_written_name_from(s)?;
            if let Some((sc, cc)) = s
                .template_content()
                .and_then(|sc| Some((sc, c.template_content()?)))
            {
                for child in sc.children() {
                    let Some(imp) = doc.import_node(child, true) else {
                        // Lexbor could not copy a content child. Giving up
                        // here leaves the clone's template SHORT, which is
                        // the truncated answer the contract forbids.
                        return Err(AdapterOom);
                    };
                    cc.insert_child(imp);
                }
                stack.falloc_push((sc, cc)).map_err(|()| AdapterOom)?;
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
    /// The tree grew deeper than the [`DepthLimit`] allowed.
    TooDeep,
    /// A `<select>` received more than `MAX_SELECT_OPTIONS` options.
    TooManyOptions,
}

impl FragmentError {
    /// The message, for every error but [`TooDeep`](FragmentError::TooDeep)
    /// and [`TooManyOptions`](FragmentError::TooManyOptions), whose messages
    /// name their limit and are the bridge's to word.
    pub fn message(self) -> &'static str {
        match self {
            FragmentError::Parser => "failed to create HTML parser",
            FragmentError::Decode => "out of memory decoding fragment HTML",
            FragmentError::Parse => "failed to parse HTML fragment",
            FragmentError::TooDeep => "document tree depth limit exceeded",
            FragmentError::TooManyOptions => "too many option elements in one select element",
        }
    }
}

/// Deep-import each child of `root` into `doc`, appending each to `into`.
///
/// `Err` when a child could not be copied whole. It REPORTS rather than
/// raising, and that still matters now the C has gone: every caller owns the
/// transient document the fragment was parsed into
/// (`lexbor::abi::TransientDoc`), and a raise from here would longjmp past its
/// `Drop` - one leaked Lexbor document per failure. The caller raises once its
/// own cleanup has run, with the message that suits it.
unsafe fn import_fragment_children(
    doc: RawDoc,
    root: RawNode,
    into: RawNode,
) -> Result<(), AdapterOom> {
    let into = BuildingNode::from_raw_node(into);
    let hdoc = doc.as_doc();
    /* `children` reads each next sibling before yielding the node; import does
     * not unlink the source anyway. */
    for child in root.as_node().children() {
        let imp = import_fixed(hdoc, child, true)?;
        into.insert_child(imp);
    }
    Ok(())
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
    /// Parse `input` in `context`, refusing a tree deeper than `limit`.
    ///
    /// # Safety
    /// `context`'s element or document must be live; `input` is only read.
    pub unsafe fn parse(
        input: &[u8],
        known_valid: bool,
        context: &FragmentContext,
        limit: DepthLimit,
    ) -> Result<TransientFragment, FragmentError> {
        run_fragment_parser(input, known_valid, context, limit)
    }

    /// Import every child into `doc`, as the children of `into`; `Err` if
    /// one failed, with the ones before it already there.
    ///
    /// Always into a detached DOCUMENT_FRAGMENT, never into a live tree: a
    /// failure part-way is then invisible, and the caller places the whole
    /// fragment afterwards (see `bridge::fragment::stage_fragment_in`).
    ///
    /// # Safety
    /// `doc` must be live, and `into` a detached fragment of `doc` that nothing
    /// else refers to.
    pub unsafe fn import_into(self, doc: RawDoc, into: RawNode) -> Result<(), AdapterOom> {
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
    Tag { doc: RawDoc, at: FragmentTag },
}

/// The context a fragment is parsed "inside of", per the WHATWG algorithm, as
/// the tag and namespace ids the parser takes - a pair that used to travel as
/// two bare `usize`s, where swapping them compiled. Declared once, here, for
/// the parser that reads it and the bridge that resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentTag {
    pub tag: TagId,
    /// `None` for an element in no namespace, which a context node can be.
    pub ns: Option<NsId>,
}

impl FragmentTag {
    /// `<body>` in the HTML namespace - the context when none is given.
    pub const BODY: FragmentTag = FragmentTag {
        tag: TagId::BODY,
        ns: Some(NsId::HTML),
    };
    /// The SVG root, `<svg>` in its own namespace.
    pub const SVG: FragmentTag = FragmentTag {
        tag: TagId::SVG,
        ns: Some(NsId::SVG),
    };
    /// The MathML root, `<math>` in its own namespace.
    pub const MATH: FragmentTag = FragmentTag {
        tag: TagId::MATH,
        ns: Some(NsId::MATH),
    };

    /// The context an element provides: its own tag and namespace. `None`
    /// only for an element Lexbor gave no tag id, which it never makes.
    pub fn of(el: HtmlElement<'_>) -> Option<FragmentTag> {
        Some(FragmentTag {
            tag: el.node().tag_id()?,
            ns: el.node().ns_id(),
        })
    }

    /// An HTML-namespace tag.
    pub fn html(tag: TagId) -> FragmentTag {
        FragmentTag {
            tag,
            ns: Some(NsId::HTML),
        }
    }
}

impl FragmentContext {
    /// What `lxb_html_parse_fragment_chunk_begin` takes: the owner document
    /// for the fragment's own, and the context's tag and namespace ids.
    ///
    /// The element context passes NO owner - as `lxb_html_parse_fragment` does,
    /// handing over a fresh parser's tree document, which is NULL - so its
    /// fragment is built in a standalone document; the tag context passes the
    /// TARGET document, so its fragment is made inside that document's memory.
    ///
    /// # Safety
    /// The context element, if that is the context, must be live.
    unsafe fn begin_args(&self) -> (*mut crate::lexbor::abi::lxb_html_document_t, usize, usize) {
        match *self {
            FragmentContext::Element(el) => {
                // SAFETY: the caller's contract - the context element is live.
                let node = unsafe { el.as_node() };
                (
                    core::ptr::null_mut(),
                    node.tag_id().map_or(0, TagId::raw),
                    node.ns_id().map_or(0, NsId::raw),
                )
            }
            /* A document handle is untyped; this entry takes the HTML document
             * it is. */
            FragmentContext::Tag { doc, at } => (
                doc.as_ptr().cast(),
                at.tag.raw(),
                at.ns.map_or(0, NsId::raw),
            ),
        }
    }
}

/// Run a fragment parse with a fresh parser, under the tree-depth guard.
///
/// Lexbor's chunked fragment API, driven by hand rather than through
/// `lxb_html_parse_fragment*` - which is exactly begin/process/end - because
/// the guard has to be installed on the tokenizer between `begin` and
/// `process`. That also puts the fragment's standalone document (the element
/// context's) in our hands BEFORE the parse, so a failed parse frees it: the
/// one-shot call abandoned it on a failure, which was one leaked document per
/// refused `inner_html=` once the limit made failures routine.
///
/// The parser is destroyed on every path, BEFORE that document: the fragment
/// tree belongs to its document, not the parser.
unsafe fn run_fragment_parser(
    input: &[u8],
    known_valid: bool,
    context: &FragmentContext,
    limit: DepthLimit,
) -> Result<TransientFragment, FragmentError> {
    /* Declared first so it drops last, after the parser. */
    let mut owned: Option<TransientDoc> = None;
    let parser = HtmlParser::create().ok_or(FragmentError::Parser)?;
    let src = sanitize(input, known_valid).ok_or(FragmentError::Decode)?;
    let bytes = src.as_slice();

    let (owner, tag, ns) = context.begin_args();
    if lxb_html_parse_fragment_chunk_begin(parser.as_ptr(), owner, tag, ns) != LXB_STATUS_OK {
        return Err(FragmentError::Parse);
    }
    /* Only the element context gets a document of its own to free; the tag
     * context's lives in the target's memory, and Lexbor's own chunk cleanup
     * destroys it. */
    if matches!(context, FragmentContext::Element(_)) {
        owned = TransientDoc::own(fragment_document(parser.as_ptr()).cast());
    }

    /* A fragment keeps one synthetic `<html>` root below its first element,
     * which the depth does not count (see `tree_guard`). */
    let mut hook = TokenHook::new(limit, 1, None);
    if !hook.install(parser.as_ptr()) {
        return Err(FragmentError::Parse); /* never unguarded */
    }
    let st = lxb_html_parse_fragment_chunk_process(parser.as_ptr(), bytes.as_ptr(), bytes.len());
    let root = if st == LXB_STATUS_OK {
        lxb_html_parse_fragment_chunk_end(parser.as_ptr())
    } else {
        core::ptr::null_mut()
    };
    hook.resume_panic(); /* none can be latched without a recorder; kept uniform */
    drop(src); /* the parse consumed it; the buffer goes on every path */
    drop(parser); /* the fragment belongs to its document, not to the parser */

    /* `owned` drops, freeing it, on either refusal. */
    match hook.stopped() {
        Some(GuardStop::TooDeep) => return Err(FragmentError::TooDeep),
        Some(GuardStop::TooManyOptions) => return Err(FragmentError::TooManyOptions),
        None => {}
    }
    let root = RawNode::from_ptr(root.cast()).ok_or(FragmentError::Parse)?;
    Ok(TransientFragment { root, _doc: owned })
}

/// Copy `src` into `doc`, `<template>` contents included, or `Err` on failure.
///
/// The DOM `importNode` omits a template's separate content fragment, so every
/// copy in this extension is import-plus-fixup; this is that one operation.
/// Three callers wanted it with different deep flags, different error channels
/// and different messages, and each had grown its own copy of the four lines -
/// so the operation lives here and they keep only the parts that differ.
pub unsafe fn import_with_fixup(
    doc: RawDoc,
    src: RawNode,
    deep: bool,
) -> Result<RawNode, AdapterOom> {
    /* SAFETY: a live document and the caller's live source node. The handles
     * do not outlive this call. */
    import_fixed(doc.as_doc(), src.as_node(), deep).map(|n| RawNode::from(n.node()))
}

/// [`import_with_fixup`] over the typed handles, for the children this module
/// walks itself.
fn import_fixed<'d>(
    hdoc: HtmlDoc<'d>,
    hsrc: HtmlNode<'_>,
    deep: bool,
) -> Result<BuildingNode<'d>, AdapterOom> {
    let himp = hdoc.import_node(hsrc, deep).ok_or(AdapterOom)?;
    if !deep {
        himp.copy_written_name_from(hsrc)?;
    }
    if deep {
        // A copy whose <template> lost its contents is a wrong answer, not a
        // degraded one: `<template><i>x</i></template>` comes back as
        // `<template></template>` and nothing says so. The C was best-effort
        // here and the port carried that over; the OOM sweep called it, which
        // is what the sweep is for.
        fixup_template_content(hdoc, hsrc, himp)?;
    }
    if hsrc.owner_document() != hdoc {
        /* The copied offsets index the other document's source. */
        himp.clear_source_offsets();
    }
    Ok(himp)
}
