//! Fragment parsing for the arena DOM (`crate::html::dom`): the fragment is
//! parsed by the ported tree builder (`crate::html::tree::parse_fragment`)
//! in an arena document of its own, in the context's tag and namespace, under
//! the same guard, and its children are then copied into the target - the
//! arena's counterpart of `lexbor::fragment`, with its API.
//!
//! A context is only ever a tag and a namespace to the fragment parser. One
//! outside the static tables - a tag past the static range, a namespace past
//! the built-in ones - decides nothing in the parse (an unknown name is no
//! special context, an unknown namespace a foreign one), so the parser is given
//! a plain `div` and no namespace in their place, and the elements that
//! inherited the namespace get it back on import, as the Lexbor DOM does.

pub use crate::lexbor::fragment::FragmentError;

use crate::falloc::{try_to_vec, VecPush};
use crate::html::dom::{
    DomOom, HtmlDoc, HtmlDocMut, HtmlElement, HtmlNode, NodeId, NodeType, NsId, RawNode, TagId,
};
use crate::html::res::names as res;
use crate::html::tree::guard::ContextOptions;
use crate::html::tree::{parse_fragment, ContextFacts, FragmentFailure, ParsedFragment};
use crate::lexbor::adapter::html as lex;
use crate::lexbor::adapter::tree_guard::{resolve_option_chain, DepthLimit, OptionContext};

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
        options: OptionContext,
        facts: ContextFacts,
    },
    /// Inside a named context, for a fragment bound to a document.
    Tag { at: FragmentTag },
}

impl FragmentContext {
    /// The context the element `el` of `doc` provides (`inner_html=`).
    pub fn element(doc: HtmlDoc<'_>, el: RawNode) -> Result<FragmentContext, FragmentError> {
        let node = doc.node(el);
        let at = match node.element() {
            Some(e) => FragmentTag::resolve(e, doc).map_err(|_| FragmentError::Context)?,
            None => None,
        };
        /* The option guard's walk into the context's ancestors, made now. */
        let chain = core::iter::successors(Some(node), |n| n.parent()).map(|n: HtmlNode<'_>| {
            let tag = n
                .tag_id()
                .and_then(TagId::static_index)
                .and_then(lex::TagId::from_raw);
            let ns = (n.ns_id() == Some(NsId::HTML)).then_some(lex::NsId::HTML);
            (tag, ns)
        });
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
        let facts = ContextFacts {
            in_form,
            compat_mode: u8::try_from(doc.compat_mode()).unwrap_or(0),
            html_integration_point: integration,
        };
        Ok(FragmentContext::Element {
            at,
            options: resolve_option_chain(chain),
            facts,
        })
    }

    /// A named context.
    pub fn tag(at: FragmentTag) -> FragmentContext {
        FragmentContext::Tag { at }
    }

    /// A named context for a fragment of `doc` - the document plays no part
    /// in the parse here; the copy lands in it on import.
    pub fn tag_in(_doc: HtmlDoc<'_>, at: FragmentTag) -> FragmentContext {
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
        let options = match context {
            FragmentContext::Element { options, .. } => match *options {
                OptionContext::Resolved {
                    fresh,
                    after_optgroup,
                } => ContextOptions {
                    fresh,
                    after_optgroup,
                },
                _ => ContextOptions::default(),
            },
            /* A named context reaches a select only by being one. */
            FragmentContext::Tag { .. } => {
                let select = tag == res::TAG_SELECT && ns == res::NS_HTML;
                ContextOptions {
                    fresh: select,
                    after_optgroup: select,
                }
            }
        };
        let facts = match context {
            FragmentContext::Element { facts, .. } => *facts,
            FragmentContext::Tag { .. } => ContextFacts::default(),
        };
        let parsed = parse_fragment(input, known_valid, tag, ns, options, facts, limit).map_err(
            |f| match f {
                FragmentFailure::Decode => FragmentError::Decode,
                FragmentFailure::Parse => FragmentError::Parse,
                FragmentFailure::Guard(stop) => FragmentError::Guard(stop),
            },
        )?;
        let inherit = match at.map(|a| &a.ns) {
            Some(FragNs::Id(Some(ns))) if !ns.is_static() => FragNs::Id(Some(*ns)),
            Some(FragNs::Uri(u)) => FragNs::Uri(try_to_vec(u).ok_or(FragmentError::Context)?),
            _ => FragNs::Id(None),
        };
        Ok(TransientFragment { parsed, inherit })
    }

    /// Copy every child into `d`'s detached fragment `into`; `Err` if one
    /// could not be copied, with the ones before it already there.
    pub fn import_into_doc(self, d: &mut HtmlDocMut<'_>, into: RawNode) -> Result<(), DomOom> {
        let inherit = match &self.inherit {
            FragNs::Id(ns) => *ns,
            FragNs::Uri(u) => Some(d.intern_ns(u).ok_or(DomOom)?),
        };
        let arena = d.arena();
        let src = &self.parsed.doc;
        let mut child = src.first_child(self.parsed.root);
        while let Some(c) = child {
            let made = arena.copy_from(src, c, true)?;
            arena.append_child(into.id(), made);
            child = src.next(c);
        }
        if let Some(ns) = inherit {
            give_namespace(d, into.id(), ns)?;
        }
        Ok(())
    }
}

/// Put every element under `root` (template contents included) that is in
/// no namespace into `ns`: the elements that inherited a context namespace
/// the parser was not given.
fn give_namespace(d: &mut HtmlDocMut<'_>, root: NodeId, ns: NsId) -> Result<(), DomOom> {
    let mut todo: Vec<NodeId> = Vec::new();
    {
        let doc = d.read();
        let Some(r) = doc.get(root) else {
            return Ok(());
        };
        for n in r.subtree_with_contents() {
            if n.node_type() == NodeType::Element && n.ns_id().is_none() {
                todo.falloc_push(n.id()).map_err(|()| DomOom)?;
            }
        }
    }
    let a = d.arena();
    for id in todo {
        let (local, prefix) = (a.local(id), a.prefix(id));
        a.set_names(id, local as usize, ns.number(), prefix);
    }
    Ok(())
}
