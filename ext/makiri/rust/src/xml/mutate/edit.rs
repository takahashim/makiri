//! Changing what one node holds - its content - as opposed to where it sits
//! (`super::insert`) or what it carries (`super::attr`). A node's name is
//! fixed once made: the DOM has no rename, and Makiri has none either.

#![forbid(unsafe_code)]

use crate::xml::chars::validate_chars;
use crate::xml::{Document, MutStatus, NodeId, NodeType};

/// Whether `text` is free of the SEQUENCE its node kind cannot hold: "--" (or
/// a trailing "-") in a comment, "]]>" in CDATA, "?>" in a PI. Each would close
/// the construct early, so the value is refused rather than escaped.
///
/// A mutation precondition, not a naming rule: the parser never needs it,
/// because it finds those sequences structurally while scanning.
pub(super) fn value_seq_ok(node_type: NodeType, text: &[u8]) -> bool {
    match node_type {
        NodeType::Comment => text.last() != Some(&b'-') && !text.windows(2).any(|w| w == b"--"),
        NodeType::CData => !text.windows(3).any(|w| w == b"]]>"),
        NodeType::Pi => !text.windows(2).any(|w| w == b"?>"),
        _ => true,
    }
}

pub fn set_content(doc: &mut Document, node: NodeId, text: &[u8]) -> Result<(), MutStatus> {
    if !validate_chars(text) {
        return Err(MutStatus::BadChars);
    }
    match doc.type_(node) {
        Some(ty @ (NodeType::Text | NodeType::CData | NodeType::Comment | NodeType::Pi)) => {
            if !value_seq_ok(ty, text) {
                return Err(MutStatus::BadChars);
            }
            doc.set_value_bytes(node, text).map_err(MutStatus::from)
        }
        Some(NodeType::Element) => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeId> = None;
            if !text.is_empty() {
                let v = doc.store(text)?;
                let n = doc.new_node(NodeType::Text)?;
                doc.node_mut(n).value = v;
                t = Some(n);
            }
            doc.replace_children(node, t);
            Ok(())
        }
        _ => Err(MutStatus::Type),
    }
}
