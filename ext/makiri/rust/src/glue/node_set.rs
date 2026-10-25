//! `Makiri::NodeSet`'s Ruby methods.
//!
//! The set itself - its private layout, Ruby-allocator storage, GC `mark` and
//! `free`, and the operations that move node pointers between sets - is
//! [`crate::bridge::node_set`]'s, and no pointer passes through here. This
//! module reads the arguments (indexes, Ranges, the `.new` list) and registers
//! the methods.

#![forbid(unsafe_code)]

use core::ffi::c_long;

use crate::bridge::rb::{prelude::*, Error, RArray, Ruby, Value};
use crate::{function, method};

use crate::bridge::node_set::{node_set_of_nodes, Membership, NodeSetSelf};
use crate::bridge::ruby::{is_kind_of, range_beg_len};
use crate::bridge::wrapper::keepalive_document;
use crate::init::{CLASS_DOCUMENT, CLASS_NODE, CLASS_NODE_SET};

fn length(s: NodeSetSelf) -> Result<usize, Error> {
    s.count()
}

/// `set[i]` -> Node or nil (negative counts from the end);
/// `set[start, length]` and `set[range]` -> a new NodeSet, nil when the start is
/// out of range. Mirrors `Array#[]`.
fn aref(ruby: &Ruby, s: NodeSetSelf, args: &[Value]) -> Result<Option<Value>, Error> {
    let count = s.count()? as c_long;
    let from_end = |i: c_long| if i < 0 { i + count } else { i };
    match args {
        [range] if range.is_kind_of(ruby.class_range()) => {
            /* A start outside the set is nil; a bound too large for a `long`
             * raises, as `Array#[]` does. The ends are Integers or nil: one
             * that is not would be converted by its `to_int`. */
            match range_beg_len(crate::bridge::ruby::integer_range_arg(*range)?, count)? {
                Some((beg, len)) => s.slice(ruby, beg as usize, len as usize),
                None => Ok(None),
            }
        }
        [index] => {
            let i = from_end(crate::bridge::ruby::long_arg(*index)?);
            if i < 0 || i >= count {
                return Ok(None);
            }
            s.at(ruby, i as usize)
        }
        [beg, len] => {
            let beg = from_end(crate::bridge::ruby::long_arg(*beg)?);
            let len = crate::bridge::ruby::long_arg(*len)?;
            if beg < 0 || beg > count || len < 0 {
                return Ok(None);
            }
            s.slice(ruby, beg as usize, len as usize)
        }
        _ => Err(crate::glue::args::arity_error(args.len(), 1, Some(2))),
    }
}

/// `each`: yields every node; an Enumerator without a block.
///
/// Iterates by index, each node read under a short borrow released before the
/// yield, so the block can call back into this set.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "the block is the point of the method; no guard or copy is held across it"
    )
)]
fn each(ruby: &Ruby, s: NodeSetSelf) -> Result<Value, Error> {
    let this = s.value();
    if !crate::bridge::raw::block_given(ruby) {
        return crate::bridge::build::enumerator(ruby, this, "each");
    }
    /* By index, not over a copy: a set is never changed once filled, and
     * a copy would be owned by this frame - which an Enumerator over
     * `each` (`set.each.next`, then dropped) suspends in its Fiber and
     * never resumes, so nothing would free it. */
    for i in 0..s.count()? {
        /* Never None while the set is unchanged; a shrunk set would be a
         * broken invariant, not a shorter answer. */
        let node = s.at(ruby, i)?.ok_or_else(|| {
            crate::bridge::ruby::internal_error("a node set changed while it was iterated")
        })?;
        crate::bridge::raw::yield_value(ruby, node)?;
    }
    Ok(this)
}

fn dup(ruby: &Ruby, s: NodeSetSelf, _args: &[Value]) -> Result<Value, Error> {
    s.copy(ruby)
}

fn op_or(ruby: &Ruby, s: NodeSetSelf, other: Value) -> Result<Value, Error> {
    s.union(ruby, &*s.operand(ruby, other)?)
}

fn op_plus(ruby: &Ruby, s: NodeSetSelf, other: Value) -> Result<Value, Error> {
    s.concat(ruby, &*s.operand(ruby, other)?)
}

fn op_and(ruby: &Ruby, s: NodeSetSelf, other: Value) -> Result<Value, Error> {
    s.filter(ruby, &*s.operand(ruby, other)?, Membership::In)
}

fn op_minus(ruby: &Ruby, s: NodeSetSelf, other: Value) -> Result<Value, Error> {
    s.filter(ruby, &*s.operand(ruby, other)?, Membership::NotIn)
}

/// `NodeSet.new(document_or_node, list = [])`.
///
/// Mirrors Nokogiri: the first argument is the owning Document (or any node,
/// whose document is taken) that the set pins as a GC keepalive; the optional
/// list seeds it, and every node in it must belong to that document.
fn s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = crate::glue::args::scan(ruby, args, crate::glue::args::Shape::positional(1, 1))?;
    let owner = a.required(0);
    let list = a.optional(0);

    let document = if is_kind_of(ruby, owner, &CLASS_DOCUMENT) {
        owner
    } else if is_kind_of(ruby, owner, &CLASS_NODE) {
        keepalive_document(ruby, owner)?
    } else {
        return Err(Error::new(
            crate::bridge::rb::Builtin::TypeError,
            "expected a Makiri::Document or Node as the first argument",
        ));
    };
    let nodes = match list {
        None => None,
        Some(v) => Some(RArray::from_value(v).ok_or_else(|| {
            Error::new(
                crate::bridge::rb::Builtin::TypeError,
                "expected an Array of nodes as the second argument",
            )
        })?),
    };
    node_set_of_nodes(ruby, document, nodes.into_iter().flat_map(|a| a.each()))
}

/// From `Init_makiri`, with the classes already defined.
pub fn init_node_set(ruby: &Ruby) -> Result<(), Error> {
    let klass = CLASS_NODE_SET.defined(ruby)?;

    /* Sets are made by queries and by `.new` below, never allocated bare:
     * `define_data_class` took the allocator away. */
    klass.define_singleton_method("new", function!(s_new, -1))?;

    for (name, f) in [
        ("|", method!(op_or, 1)),
        ("+", method!(op_plus, 1)),
        ("&", method!(op_and, 1)),
        ("-", method!(op_minus, 1)),
    ] {
        klass.define_method(name, f)?;
    }
    klass.define_method("length", method!(length, 0))?;
    klass.define_method("[]", method!(aref, -1))?;
    klass.define_method("each", method!(each, 0))?;
    klass.define_method("dup", method!(dup, -1))?;
    Ok(())
}
