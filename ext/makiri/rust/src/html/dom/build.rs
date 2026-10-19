//! What the tree builder (`crate::html::tree`) writes that no DOM operation
//! does: text appended to a node it already made, attributes made from an id
//! of the qualified-name table rather than from text, and Lexbor's own
//! `id`/`class` bookkeeping on append.

use super::{Document, DomOom, NodeId, NsIndex, Payload, PrefixIndex, Span};

/// Where a new attribute's value comes from.
#[derive(Clone, Copy)]
pub enum AttrValue<'a> {
    /// These bytes.
    Bytes(&'a [u8]),
    /// A copy of another attribute's value, in this document.
    CopyOf(NodeId),
}

impl Document {
    /// Append `b` to the data of the Text node `n` (`lexbor_str_append` onto
    /// its character data), as [`ByteStore::append`](super::store::ByteStore)
    /// places it.
    pub fn append_text(&mut self, n: NodeId, b: &[u8]) -> Result<(), DomOom> {
        let Some(Payload::Data(span)) = self.node(n).map(|x| x.payload) else {
            return Ok(());
        };
        let new = self.bytes.append(n, span, b)?;
        if let Some(x) = self.node_mut(n) {
            x.payload = Payload::Data(new);
        }
        Ok(())
    }

    /// A new attribute, not on any element, whose qualified name as written
    /// is `written`, an id of [`Names::qualified`](crate::html::names::Names)
    /// (0 for none).
    pub fn create_attr_by_ids(
        &mut self,
        local: usize,
        ns: NsIndex,
        prefix: PrefixIndex,
        written: u32,
        value: AttrValue<'_>,
    ) -> Result<NodeId, DomOom> {
        let value = match value {
            AttrValue::Bytes(b) => self.store(b)?,
            AttrValue::CopyOf(from) => match self.attr(from) {
                Some(a) => {
                    let v = a.value;
                    self.dup(v)?
                }
                None => Span::default(),
            },
        };
        self.push_attr(local, ns, prefix, written, value)
    }

    /// An element's or attribute's written-name id (0 for none).
    pub fn written_id(&self, n: NodeId) -> u32 {
        match self.node(n).map(|x| x.payload) {
            Some(Payload::Element(i)) => self.elements.get(i as usize).map_or(0, |e| e.written),
            Some(Payload::Attr(i)) => self.attrs.get(i as usize).map_or(0, |a| a.written),
            _ => 0,
        }
    }
}
