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

use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::falloc::OomOption;
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
use crate::lexbor::adapter::tree_guard::{
    fragment_document, DepthLimit, GuardStop, OptionContext, TokenHook,
};

/* The HTML parser's lifecycle, from the generated bindings. Declared here first
 * over an opaque parser, which was fine until the source-location port needed
 * the tokenizer inside it and build.rs started generating them - two Rust types
 * for one symbol again. */
use crate::lexbor::abi::HtmlParser;
use crate::utf8_input::sanitize;

/// `lxb_dom_document_import_node` deep-clones the normal child chain but NOT a
/// `<template>`'s separate content fragment, so an imported template comes out
/// empty. Walk source and clone in lockstep and import each template's content
/// into the clone's.
///
/// The walk is `preorder_next_with_contents`, which enters every template's
/// contents: a template's contents are imported into the clone's the moment
/// the walk reaches it, before either side steps into them, so the two walks
/// go on matching 1:1 inside the contents too - which is how a template nested
/// in another's contents gets its own contents in turn.
///
/// The same lockstep walk gives each copied element the name its source
/// records as written, which Lexbor's copy also leaves out
/// (`BuildingNode::copy_written_name_from`).
///
/// Stack-free, like the walk: an adversarially deep fragment must not be able
/// to overflow the stack. Fail-closed on allocation failure: a template content
/// that could not be copied refuses the whole import rather than leaving a
/// clone that is silently short.
///
/// `template_content` answers in one what this used to ask in three: it is
/// `None` for a node that is not an HTML `<template>` AND for one Lexbor gave no
/// contents fragment. The walk tells those apart where it matters: a source
/// template with contents whose copy has none (or the reverse) would make the
/// two walks part ways, so it refuses the copy. `cross_import`'s
/// `h2x_first_child`, where an empty template and a non-template mean
/// DIFFERENT children, keeps a test of its own.
fn fixup_template_content(
    doc: HtmlDoc<'_>,
    root_src: HtmlNode<'_>,
    root_clone: BuildingNode<'_>,
) -> Result<(), AdapterOom> {
    let (mut sn, mut cn) = (Some(root_src), Some(root_clone));
    while let (Some(s), Some(c)) = (sn, cn) {
        c.copy_written_name_from(s)?;
        /* The clone-side test is only worth paying for once the source side
         * has said this is a template: every node of every deep import passes
         * through here. */
        match s.template_content() {
            Some(sc) => {
                let cc = c.template_content().or_oom()?;
                for child in sc.children() {
                    let Some(imp) = doc.import_node(child, true) else {
                        // Lexbor could not copy a content child. Giving up
                        // here leaves the clone's template SHORT, which is
                        // the truncated answer the contract forbids.
                        return Err(AdapterOom);
                    };
                    cc.insert_child(imp);
                }
            }
            /* A template Lexbor gave no contents: its copy must have none. */
            None if s.is_html_template() && c.template_content().is_some() => {
                return Err(AdapterOom);
            }
            None => {}
        }
        sn = s.preorder_next_with_contents(root_src);
        cn = c.preorder_next_with_contents(root_clone);
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
    /// The context's tag or namespace could not be interned in the document.
    Context,
    /// The parse guard refused it ([`GuardStop`]), as it refuses a document.
    Guard(GuardStop),
}

impl From<GuardStop> for FragmentError {
    fn from(stop: GuardStop) -> Self {
        FragmentError::Guard(stop)
    }
}

impl FragmentError {
    /// The message, for every error but [`Guard`](FragmentError::Guard),
    /// whose message names its limit and is the bridge's to word
    /// (`bridge::doc::guard_error`, shared with a document parse).
    pub fn message(self) -> &'static str {
        match self {
            FragmentError::Parser => "failed to create HTML parser",
            FragmentError::Decode => "out of memory decoding fragment HTML",
            FragmentError::Parse => "failed to parse HTML fragment",
            FragmentError::Context => "failed to resolve the fragment context",
            FragmentError::Guard(_) => "the parse guard refused the fragment",
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
    /// The context's namespace when the parser was given none in its place
    /// (see [`FragmentTag::parse_ns`]): the elements that inherited it are
    /// given it back as they are imported.
    inherit: Option<NsId>,
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
        import_fragment_children(doc, self.root, into)?;
        if let Some(ns) = self.inherit {
            BuildingNode::from_raw_node(into).give_namespace(ns);
        }
        Ok(())
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

/// The tag a context gets when its own cannot be handed to the parser - see
/// [`FragmentTag::resolve`]. No element can be named by it (a DOM element name
/// does not start with `#`), so it is a tag of the document's own that no
/// element carries and no static entry has.
const STANDIN_TAG: &[u8] = b"#fragment-context";

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

    /// The context the element `el` provides, as ids of `doc` - the document
    /// the fragment is parsed into or, for an element context, `el`'s own.
    /// `Ok(None)` only for an element Lexbor gave no tag id, which it never
    /// makes; `Err` when an id could not be interned.
    ///
    /// The ids are not simply `el`'s. A namespace or tag `el`'s document
    /// interned is a pointer into THAT document's table, meaningless in `doc`
    /// and dangling once `el`'s document is gone:
    ///
    /// - such a namespace is interned in `doc` (a no-op when `el` is `doc`'s),
    ///   to be given to what inherits it (see [`parse_ns`](Self::parse_ns));
    /// - such a tag - one not in Lexbor's static table - becomes
    ///   [`STANDIN_TAG`], a tag of `doc`'s own that the parser does not know
    ///   either. It decides nothing: a context of an unknown name is no
    ///   special context, in any namespace.
    pub fn resolve(
        el: HtmlElement<'_>,
        doc: HtmlDoc<'_>,
    ) -> Result<Option<FragmentTag>, AdapterOom> {
        let node = el.node();
        let Some(tag) = node.tag_id() else {
            return Ok(None);
        };
        let ns = match node.ns_id() {
            Some(ns) if !ns.is_static() && node.owner_document() != doc => {
                Some(doc.intern_ns(node.ns_uri().or_oom()?).or_oom()?)
            }
            ns => ns,
        };
        let tag = match tag.static_index() {
            Some(_) => tag,
            None => doc.intern_tag(STANDIN_TAG).or_oom()?,
        };
        Ok(Some(FragmentTag { tag, ns }))
    }

    /// The namespace the parser is given: the context's own when it is one of
    /// Lexbor's built-in namespaces, and NONE in place of any other.
    ///
    /// The parse is the same either way - the tree builder tells HTML, SVG
    /// and MathML apart and nothing else, so an element in no namespace and
    /// one in `urn:x` are the same foreign context to it. Lexbor is handed
    /// only the namespaces it builds in (hardening), and
    /// [`inherit`](Self::inherit) is given to the elements that inherited the
    /// context's on import.
    fn parse_ns(self) -> usize {
        match self.ns {
            Some(ns) if ns.is_static() => ns.raw(),
            _ => 0,
        }
    }

    /// The namespace [`parse_ns`](Self::parse_ns) held back, if any.
    fn inherit(self) -> Option<NsId> {
        self.ns.filter(|ns| !ns.is_static())
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
    /// for the fragment's own, and the context's tag and namespace ids - the
    /// namespace as [`FragmentTag::parse_ns`] gives it, with the one it held
    /// back.
    ///
    /// The element context passes NO owner - as `lxb_html_parse_fragment` does,
    /// handing over a fresh parser's tree document, which is NULL - so its
    /// fragment is built in a standalone document; the tag context passes the
    /// TARGET document, so its fragment is made inside that document's memory.
    ///
    /// The element context is resolved here, against its own document (see
    /// [`FragmentTag::resolve`]); a tag context arrives resolved. `Err` when
    /// that resolution could not intern an id.
    ///
    /// # Safety
    /// The context element, if that is the context, must be live.
    unsafe fn begin_args(&self) -> Result<BeginArgs, FragmentError> {
        let (owner, at) = match *self {
            FragmentContext::Element(el) => {
                // SAFETY: the caller's contract - the context element is live.
                let node = unsafe { el.as_node() };
                let at = match node.element() {
                    Some(e) => FragmentTag::resolve(e, node.owner_document())
                        .map_err(|_| FragmentError::Context)?,
                    None => None,
                };
                (core::ptr::null_mut(), at)
            }
            /* A document handle is untyped; this entry takes the HTML document
             * it is. */
            FragmentContext::Tag { doc, at } => (doc.as_ptr().cast(), Some(at)),
        };
        Ok(BeginArgs {
            owner,
            tag: at.map_or(0, |at| at.tag.raw()),
            ns: at.map_or(0, FragmentTag::parse_ns),
            inherit: at.and_then(FragmentTag::inherit),
        })
    }
}

/// [`FragmentContext::begin_args`]'s answer.
struct BeginArgs {
    owner: *mut crate::lexbor::abi::lxb_html_document_t,
    tag: usize,
    ns: usize,
    inherit: Option<NsId>,
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

    let BeginArgs {
        owner,
        tag,
        ns,
        inherit,
    } = context.begin_args()?;
    if lxb_html_parse_fragment_chunk_begin(parser.as_ptr(), owner, tag, ns) != LXB_STATUS_OK {
        return Err(FragmentError::Parse);
    }
    /* Only the element context gets a document of its own to free; the tag
     * context's lives in the target's memory, and Lexbor's own chunk cleanup
     * destroys it. */
    if matches!(context, FragmentContext::Element(_)) {
        owned = TransientDoc::own(fragment_document(parser.as_ptr()).cast());
    }

    let mut hook = TokenHook::for_fragment(
        limit,
        match *context {
            FragmentContext::Element(el) => OptionContext::Element(el),
            FragmentContext::Tag { at, .. } => OptionContext::Tag(Some(at.tag), at.ns),
        },
    );
    if !hook.install(parser.as_ptr()) {
        return Err(FragmentError::Parse); /* never unguarded */
    }
    let st = lxb_html_parse_fragment_chunk_process(parser.as_ptr(), bytes.as_ptr(), bytes.len());
    let root = if st == LXB_STATUS_OK {
        lxb_html_parse_fragment_chunk_end(parser.as_ptr())
    } else {
        core::ptr::null_mut()
    };
    /* `owned` drops, freeing it, on a refusal - after the parser, which was
     * declared after it. */
    hook.finish()?;
    drop(src); /* the parse consumed it; the buffer goes on every path */
    drop(parser); /* the fragment belongs to its document, not to the parser */
    let root = RawNode::from_ptr(root.cast()).ok_or(FragmentError::Parse)?;
    Ok(TransientFragment {
        root,
        _doc: owned,
        inherit,
    })
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
    let himp = hdoc.import_node(hsrc, deep).or_oom()?;
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
