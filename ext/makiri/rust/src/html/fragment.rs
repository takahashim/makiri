//! Fragment parsing for the arena DOM (`crate::html::dom`): the fragment is
//! parsed by the ported tree builder (`crate::html::tree::parse_fragment`)
//! in an arena document of its own, in the context's tag and namespace, under
//! the same guard, and its children are then copied into the target - the
//! arena's counterpart of `lexbor::fragment`, with its API.
//!
//! A context is a tag and a namespace to the fragment parser, and the few
//! other things it reads from the element and its document
//! (`ContextFacts`). A tag or namespace outside the static tables - a tag
//! past the static range, a namespace past the built-in ones - is given as a
//! plain `div` and no namespace in its place: an unknown name is no special
//! context, and an unknown namespace is as foreign as none, which
//! `ContextFacts::foreign_namespace` tells the parser. The elements that
//! inherited the namespace get it back on import, as the Lexbor DOM does.

use crate::falloc::try_to_vec;
use crate::html::dom::{
    DomOom, HtmlDoc, HtmlElement, HtmlNewNodes, HtmlNode, NsId, RawNode, TagId,
};
use crate::html::res::names as res;
use crate::html::tree::guard::{DepthLimit, GuardStop};
use crate::html::tree::{
    parse_fragment, ContextElement, ContextFacts, FragmentFailure, ParsedFragment,
};

/// Why a fragment parse produced no fragment. The bridge words it for Ruby.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentError {
    /// Lexbor could not create or initialise a parser (Lexbor's DOM only).
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

/// A context's namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FragNs {
    /// An id of the target document, or none.
    Id(Option<NsId>),
    /// A namespace of another document the target has not interned: its URI,
    /// interned when the fragment is imported.
    Uri(Vec<u8>),
}

/// The context a fragment is parsed "inside of": a tag and a namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentTag {
    tag: TagId,
    ns: FragNs,
}

impl FragmentTag {
    /// `<body>` in the HTML namespace.
    pub const BODY: FragmentTag = FragmentTag {
        tag: TagId::BODY,
        ns: FragNs::Id(Some(NsId::HTML)),
    };
    /// The SVG root.
    pub const SVG: FragmentTag = FragmentTag {
        tag: TagId::SVG,
        ns: FragNs::Id(Some(NsId::SVG)),
    };
    /// The MathML root.
    pub const MATH: FragmentTag = FragmentTag {
        tag: TagId::MATH,
        ns: FragNs::Id(Some(NsId::MATH)),
    };

    /// An HTML-namespace tag.
    pub fn html(tag: TagId) -> FragmentTag {
        FragmentTag {
            tag,
            ns: FragNs::Id(Some(NsId::HTML)),
        }
    }

    /// The context the element `el` provides to a fragment parsed into
    /// `target`. `Ok(None)` for an element with no tag; `Err` when a
    /// namespace URI could not be copied.
    pub fn resolve(
        el: HtmlElement<'_>,
        target: HtmlDoc<'_>,
    ) -> Result<Option<FragmentTag>, DomOom> {
        let node = el.node();
        let Some(tag) = node.tag_id() else {
            return Ok(None);
        };
        let ns = match node.ns_id() {
            Some(ns) if !ns.is_static() && node.owner_document() != target => {
                let uri = node.ns_uri().ok_or(DomOom)?;
                match target.lookup_ns(uri) {
                    Some(id) => FragNs::Id(Some(id)),
                    None => FragNs::Uri(try_to_vec(uri).ok_or(DomOom)?),
                }
            }
            ns => FragNs::Id(ns),
        };
        Ok(Some(FragmentTag { tag, ns }))
    }

    /// The tag the parser is given: the same static id, or `div`.
    fn parse_tag(&self) -> usize {
        self.tag.static_index().unwrap_or(res::TAG_DIV)
    }

    /// The namespace the parser is given: a built-in one, or none (0).
    fn parse_ns(&self) -> usize {
        match &self.ns {
            FragNs::Id(Some(ns)) if ns.is_static() => ns.number() as usize,
            _ => 0,
        }
    }
}

/// Where a fragment is parsed.
pub enum FragmentContext {
    /// Inside the element whose context was resolved beforehand.
    Element {
        at: Option<FragmentTag>,
        facts: ContextFacts,
    },
    /// Inside a named context, for a fragment bound to a document.
    Tag { at: FragmentTag },
}

impl FragmentContext {
    /// The context the element `el` of `doc` provides (`inner_html=`).
    pub fn element(doc: HtmlDoc<'_>, el: RawNode) -> Result<FragmentContext, FragmentError> {
        FragmentContext::element_into(doc.node(el), doc)
    }

    /// The context `node` provides to a fragment made in `target`, which need
    /// not be `node`'s document (`DocumentFragment.parse(html, context: el)`).
    /// The HTML Standard's fragment parsing reads the element whole: its
    /// document's quirks mode, its ancestors' form, its attributes - so
    /// `Node#parse` and `context:` answer as `inner_html=` does.
    pub fn element_into(
        node: HtmlNode<'_>,
        target: HtmlDoc<'_>,
    ) -> Result<FragmentContext, FragmentError> {
        let at = match node.element() {
            Some(e) => FragmentTag::resolve(e, target).map_err(|_| FragmentError::Context)?,
            None => None,
        };
        let integration = node.ns_id() == Some(NsId::MATH)
            && node.tag_id().and_then(TagId::static_index) == Some(res::TAG_ANNOTATION_XML)
            && node
                .element()
                .and_then(|e| e.get_attribute(b"encoding"))
                .is_some_and(|v| {
                    v.eq_ignore_ascii_case(b"text/html")
                        || v.eq_ignore_ascii_case(b"application/xhtml+xml")
                });
        let in_form = core::iter::successors(Some(node), |n| n.parent()).any(|n| {
            n.ns_id() == Some(NsId::HTML)
                && n.tag_id().and_then(TagId::static_index) == Some(res::TAG_FORM)
        });
        let foreign_namespace = at.as_ref().is_some_and(|a| a.parse_ns() == 0);
        let facts = ContextFacts {
            foreign_namespace,
            in_form,
            compat_mode: u8::try_from(node.owner_document().compat_mode()).unwrap_or(0),
            html_integration_point: integration,
        };
        Ok(FragmentContext::Element { at, facts })
    }

    /// A named context.
    pub fn tag(at: FragmentTag) -> FragmentContext {
        FragmentContext::Tag { at }
    }

    fn at(&self) -> Option<&FragmentTag> {
        match self {
            FragmentContext::Element { at, .. } => at.as_ref(),
            FragmentContext::Tag { at } => Some(at),
        }
    }
}

/// A fragment parsed into an arena document of its own, not yet copied into
/// the target.
pub struct TransientFragment {
    parsed: ParsedFragment,
    /// The context's namespace when the parser was given none in its place.
    inherit: FragNs,
}

impl TransientFragment {
    /// Parse `input` in `context`, refusing a tree deeper than `limit`.
    pub fn parse(
        input: &[u8],
        known_valid: bool,
        context: &FragmentContext,
        limit: DepthLimit,
    ) -> Result<TransientFragment, FragmentError> {
        let at = context.at();
        let (tag, ns) = at.map_or((res::TAG__UNDEF, 0), |a| (a.parse_tag(), a.parse_ns()));
        let element = match context {
            FragmentContext::Element { facts, .. } => ContextElement {
                tag,
                ns,
                facts: *facts,
            },
            FragmentContext::Tag { .. } => ContextElement::named(tag, ns),
        };
        let parsed = parse_fragment(input, known_valid, &element, limit).map_err(|f| match f {
            FragmentFailure::Decode => FragmentError::Decode,
            FragmentFailure::Oom | FragmentFailure::Stopped(_) => FragmentError::Parse,
            FragmentFailure::Guard(stop) => FragmentError::Guard(stop),
        })?;
        let inherit = match at.map(|a| &a.ns) {
            Some(FragNs::Id(Some(ns))) if !ns.is_static() => FragNs::Id(Some(*ns)),
            Some(FragNs::Uri(u)) => FragNs::Uri(try_to_vec(u).ok_or(FragmentError::Context)?),
            _ => FragNs::Id(None),
        };
        Ok(TransientFragment { parsed, inherit })
    }

    /// Copy every child into `d`'s detached fragment `into`; `Err` if one
    /// could not be copied, with the ones before it already there.
    pub fn import_into_doc(self, d: &mut HtmlNewNodes<'_>, into: RawNode) -> Result<(), DomOom> {
        let inherit = match &self.inherit {
            FragNs::Id(ns) => *ns,
            FragNs::Uri(u) => Some(d.intern_ns(u).ok_or(DomOom)?),
        };
        let src = HtmlDoc::of(&self.parsed.doc);
        let mut child = src.get(self.parsed.root).and_then(HtmlNode::first_child);
        while let Some(c) = child {
            let made = d.import(c, true)?;
            d.append_to_detached(into, made);
            child = c.next();
        }
        if let Some(ns) = inherit {
            d.give_namespace(into, ns)?;
        }
        Ok(())
    }
}
