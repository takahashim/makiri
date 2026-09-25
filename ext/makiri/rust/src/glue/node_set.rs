//! `Makiri::NodeSet`'s Ruby methods.
//!
//! The set itself - its private layout, Ruby-allocator storage, GC `mark` and
//! `free`, and the operations that move node pointers between sets - is
//! [`crate::bridge::node_set`]'s, and no pointer passes through here. This
//! module reads the arguments (indexes, Ranges, the `.new` list) and registers
//! the methods.

#![forbid(unsafe_code)]

use core::ffi::c_long;

use magnus::{function, method, prelude::*, Error, RArray, Ruby, TryConvert, Value};

use crate::bridge::node_set::{node_set_of_nodes, Membership, NodeSet};
use crate::bridge::ruby::{is_kind_of, range_beg_len};
use crate::bridge::wrapper::keepalive_document;
use crate::init::{CLASS_DOCUMENT, CLASS_NODE, CLASS_NODE_SET};

fn length(s: &NodeSet) -> Result<usize, Error> {
    crate::bridge::ruby::entry(|| s.count())
}

fn arity_error(ruby: &Ruby, given: usize) -> Error {
    Error::new(
        ruby.exception_arg_error(),
        format!("wrong number of arguments (given {given}, expected 1..2)"),
    )
}

/// `set[i]` -> Node or nil (negative counts from the end);
/// `set[start, length]` and `set[range]` -> a new NodeSet, nil when the start is
/// out of range. Mirrors `Array#[]`.
fn aref(ruby: &Ruby, s: &NodeSet, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let count = s.count()? as c_long;
        let from_end = |i: c_long| if i < 0 { i + count } else { i };
        let nil = ruby.qnil().as_value();
        match args {
            [range] if range.is_kind_of(ruby.class_range()) => {
                /* A start outside the set is nil; a bound too large for a `long`
                 * raises, as `Array#[]` does. */
                match range_beg_len(*range, count)? {
                    Some((beg, len)) => s.slice(ruby, beg as usize, len as usize),
                    None => Ok(nil),
                }
            }
            [index] => {
                let i = from_end(c_long::try_convert(*index)?);
                if i < 0 || i >= count {
                    return Ok(nil);
                }
                s.at(ruby, i as usize)
            }
            [beg, len] => {
                let beg = from_end(c_long::try_convert(*beg)?);
                let len = c_long::try_convert(*len)?;
                if beg < 0 || beg > count || len < 0 {
                    return Ok(nil);
                }
                s.slice(ruby, beg as usize, len as usize)
            }
            _ => Err(arity_error(ruby, args.len())),
        }
    })
}

/// `each`: yields every node; an Enumerator without a block.
///
/// Iterates a snapshot: the block can call back into this set (even grow it
/// through a query), and holding the borrow across the yield would turn that
/// into an error for no reason.
fn each(ruby: &Ruby, s: &NodeSet) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        /* magnus hands methods a `&NodeSet`, not the object, and `each` needs the
         * object both to return and to enumeratorize. */
        let this = crate::bridge::ruby::current_receiver()?;
        if !ruby.block_given() {
            return Ok(this.enumeratorize("each", ()).as_value());
        }
        for node in s.snapshot(ruby)?.wrapped() {
            let _: Value = ruby.yield_value(node)?;
        }
        Ok(this)
    })
}

fn dup(ruby: &Ruby, s: &NodeSet, _args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| s.slice(ruby, 0, s.count()?))
}

fn op_or(ruby: &Ruby, s: &NodeSet, other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| s.union(ruby, s.operand(ruby, other)?))
}

fn op_plus(ruby: &Ruby, s: &NodeSet, other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| s.concat(ruby, s.operand(ruby, other)?))
}

fn op_and(ruby: &Ruby, s: &NodeSet, other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| s.filter(ruby, s.operand(ruby, other)?, Membership::In))
}

fn op_minus(ruby: &Ruby, s: &NodeSet, other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| s.filter(ruby, s.operand(ruby, other)?, Membership::NotIn))
}

/// `NodeSet.new(document_or_node, list = [])`.
///
/// Mirrors Nokogiri: the first argument is the owning Document (or any node,
/// whose document is taken) that the set pins as a GC keepalive; the optional
/// list seeds it, and every node in it must belong to that document.
fn s_new(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
        let (owner,) = a.required;
        let (list,) = a.optional;

        let document = if is_kind_of(owner, &CLASS_DOCUMENT) {
            owner
        } else if is_kind_of(owner, &CLASS_NODE) {
            keepalive_document(owner)?
        } else {
            return Err(Error::new(
                ruby.exception_type_error(),
                "expected a Makiri::Document or Node as the first argument",
            ));
        };
        let nodes = match list.filter(|v| !v.is_nil()) {
            None => None,
            Some(v) => Some(RArray::from_value(v).ok_or_else(|| {
                Error::new(
                    ruby.exception_type_error(),
                    "expected an Array of nodes as the second argument",
                )
            })?),
        };
        node_set_of_nodes(
            ruby,
            document,
            nodes.into_iter().flat_map(|a| a.into_iter()),
        )
    })
}

/// From `Init_makiri`, with the classes already defined.
pub fn init_node_set() -> Result<(), Error> {
    let klass = CLASS_NODE_SET.class();

    /* Sets are made by queries and by `.new` below, never allocated bare. */
    klass.undef_default_alloc_func();
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
