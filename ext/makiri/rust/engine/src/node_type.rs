//! The DOM node type, as one enum every layer reads.
//!
//! The XPath engine branches on it, the HTML adapter answers it for a Lexbor
//! node, and the Ruby boundary picks a node's class by it. It sits at the crate
//! root, beside `token`, because none of those layers owns it: the adapter
//! reading it from the engine's module would make Lexbor's reader depend on
//! the query engine. The XML arena keeps its own narrower `xml::model::ArenaKind`
//! (it has no `Other`, since it never stores an unknown kind - and it names the
//! variants exactly as this enum does) and converts into this one with `From`.

#![forbid(unsafe_code)]

/// A node's type.
///
/// The discriminants are the WHATWG DOM numbers (`Node.nodeType`), which are
/// also Lexbor's `LXB_DOM_NODE_TYPE_*` - the HTML adapter
/// (`lexbor::adapter::html`) asserts that at compile time - so a reader holding
/// the number converts with [`from_u32`](Self::from_u32), a range check. Entity / entity reference /
/// notation have no node in either representation, but a host's number could
/// still say so, and the `node()` test has to refuse them by name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum NodeType {
    /// Anything else: a number neither DOM defines (Lexbor's UNDEF among
    /// them), or an XML node no longer in its arena. It passes `node()` and
    /// nothing else.
    Other = 0,
    Element = 1,
    Attribute = 2,
    Text = 3,
    CDataSection = 4,
    EntityReference = 5,
    Entity = 6,
    Pi = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    DocumentFragment = 11,
    Notation = 12,
}

impl NodeType {
    /// The type a DOM node-type number names; [`Other`](Self::Other) for one
    /// outside 1..=12, never an assumed kind.
    #[inline]
    pub fn from_u32(v: u32) -> NodeType {
        match v {
            1 => NodeType::Element,
            2 => NodeType::Attribute,
            3 => NodeType::Text,
            4 => NodeType::CDataSection,
            5 => NodeType::EntityReference,
            6 => NodeType::Entity,
            7 => NodeType::Pi,
            8 => NodeType::Comment,
            9 => NodeType::Document,
            10 => NodeType::DocumentType,
            11 => NodeType::DocumentFragment,
            12 => NodeType::Notation,
            _ => NodeType::Other,
        }
    }
}
