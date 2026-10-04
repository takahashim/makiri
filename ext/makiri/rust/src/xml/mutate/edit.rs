//! Changing what one node holds - its content - as opposed to where it sits
//! (`super::insert`) or what it carries (`super::attr`). A node's name is
//! fixed once made: the DOM has no rename, and Makiri has none either.

#![forbid(unsafe_code)]

use crate::xml::{ArenaKind, Document, MutError, NodeId};

/// The DOM's refusal of `data` for a new node of kind `ty`, if it refuses it:
/// `]]>` in a CDATA section, `?>` in a processing instruction - the only data
/// `createCDATASection` and `createProcessingInstruction` refuse. A comment's
/// `--`, and every character, the DOM takes; what XML cannot write is refused
/// by the serializers instead (`serialize::Failure::UnwritableData`).
///
/// A factory's precondition, not a setter's: the DOM's `data` setter checks
/// nothing, so `content=` does not either.
pub(super) fn dom_refuses_data(ty: ArenaKind, data: &[u8]) -> Option<&'static str> {
    match ty {
        ArenaKind::CDataSection if data.windows(3).any(|w| w == b"]]>") => {
            Some("CDATA section data must not contain ]]>")
        }
        ArenaKind::Pi if data.windows(2).any(|w| w == b"?>") => {
            Some("processing instruction data must not contain ?>")
        }
        _ => None,
    }
}

/// The DOM's `textContent` / `data` setter: any text, as the DOM takes it. What
/// XML cannot write (a character outside XML's, `--` in a comment, `?>` in a
/// PI) is refused by the serializers, not here: the mutators used to refuse
/// it, which the DOM does not, and a browser's tree could not be built.
pub fn set_content(doc: &mut Document, node: NodeId, text: &[u8]) -> Result<(), MutError> {
    match doc.type_(node) {
        Some(ArenaKind::Text | ArenaKind::CDataSection | ArenaKind::Comment | ArenaKind::Pi) => {
            doc.set_value_bytes(node, text).map_err(MutError::from)
        }
        /* An Attr's content is its value, by `[]=`'s rule for one it finds. */
        Some(ArenaKind::Attribute) => super::attr::set_existing_value(doc, node, text),
        Some(ArenaKind::Element) => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeId> = None;
            if !text.is_empty() {
                let v = doc.store(text)?;
                let n = doc.new_node(ArenaKind::Text)?;
                doc.node_mut(n).value = v;
                t = Some(n);
            }
            doc.replace_children(node, t);
            Ok(())
        }
        _ => Err(MutError::Type),
    }
}
