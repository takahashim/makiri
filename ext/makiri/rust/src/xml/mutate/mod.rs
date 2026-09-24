//! Mutation primitives. Every primitive validates and allocates BEFORE
//! changing any link, so a failure leaves the tree untouched.
//!
//! The tree is an index arena, so this module is ordinary safe Rust under
//! `#![forbid(unsafe_code)]`: nodes are [`NodeId`] values, structure lives in
//! the [`Document`], and names/values are spans into its byte store.
//!
//! One concern per submodule: [`insert`] decides where a node goes, [`attr`]
//! what an element carries, [`edit`] what one node is, [`factory`] builds a
//! detached one, [`ns`] decides a namespace URI, and [`copy`] duplicates a
//! subtree. Only the two helpers below are shared by more than one of them.

#![forbid(unsafe_code)]

mod attr;
mod copy;
mod edit;
mod factory;
mod insert;
mod ns;
pub use ns::{ignored_default_decl, namespace_in_scope};

use crate::xml::qname::Split;
use crate::xml::{Document, MutStatus, NodeId, Status};

pub use attr::{remove_attribute, remove_attribute_ns, set_attribute, set_attribute_ns};
pub use copy::{clone_node, copy_node_from, import_subtree};
pub use edit::set_content;
pub use factory::{
    new_chardata, new_document_type, new_element, new_fragment, new_loose_dom_element, new_pi,
};
pub use insert::{
    detach, insert_after, insert_before, insert_child, place, remove, replace_node,
    replace_with_fragment, Place,
};

/// Copy a node's span out of the arena before taking `&mut doc`. `to_vec` would
/// abort on OOM; this path must fail closed instead, like every other
/// allocation here.
pub(super) fn copy_span(bytes: &[u8]) -> Result<Vec<u8>, MutStatus> {
    crate::falloc::try_to_vec(bytes).ok_or(MutStatus::Oom)
}

/// An arena result as a mutation result, KEEPING the reason.
///
/// The one place the two status domains meet. It matters that it is one place:
/// every site used to write `.map_err(|_| MutStatus::Oom)`, which reported a
/// document's own `max_bytes`/`max_nodes` refusal as the machine running out of
/// memory. `tree::Parser::arena` is the same conversion on the parse side, and
/// it never lost the reason.
#[inline]
pub(super) fn arena<T>(r: Result<T, Status>) -> Result<T, MutStatus> {
    r.map_err(|st| match st {
        Status::Limit => MutStatus::Limit,
        /* Syntax and Unsupported are the parser's; an arena call cannot answer
         * either, so anything else here is an allocation that failed. */
        _ => MutStatus::Oom,
    })
}

#[inline]
pub(super) fn assign_qname(doc: &mut Document, node: NodeId, name: &[u8], sp: &Split) -> MutStatus {
    match arena(doc.assign_qname(node, name, sp.prefix_len, sp.local_off, sp.local_len)) {
        Ok(()) => MutStatus::Ok,
        Err(st) => st,
    }
}
