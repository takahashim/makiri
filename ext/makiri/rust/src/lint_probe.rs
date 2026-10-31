//! What the crate's own lints (`lints/makiri_lints`) must report, and what
//! they must leave alone. Compiled only under `--cfg makiri_lint_probe`, by
//! `rake dylint`, which fails unless the lints report exactly the lines that
//! end in `//~`, each by the lints named after it - so a lint that has
//! stopped seeing a case cannot pass, even where another reports the line.
#![allow(dead_code, clippy::all)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::bridge::rb::Opaque;
use crate::bridge::rb::{RString, Value};

use crate::bridge::typed::Payload;

struct Kept {
    values: Vec<(Value, u32)>, //~ ruby_value_on_heap
    marked: Opaque<Value>,
    borrowed: &'static [Value],
}

struct Data {
    value: Value, //~ ruby_value_on_heap
}

impl Payload for Data {}

type Strings = Vec<RString>;

fn parameter(_: Strings) {} //~ ruby_value_on_heap

#[rustfmt::skip] // keeps the marker on the line reported
fn returned(v: Value) -> Box<Value> { //~ ruby_value_on_heap
    Box::new(v)
}

fn bindings(v: Value) {
    let _map: HashMap<u32, Value> = HashMap::new(); //~ ruby_value_on_heap
    let _kept = vec![v]; //~ infallible_allocation ruby_value_on_heap
    let _on_stack = v;
    let _slice: &[Value] = &[];
}

fn take<T>(_: T) {}

/// Temporaries no binding names, which only the expression check sees.
#[rustfmt::skip] // keeps each marker on the line reported
fn temporaries(v: Value, kept: &Vec<Value>) {
    take(vec![v]); //~ infallible_allocation ruby_value_on_heap
    let _n = vec![v].len(); //~ infallible_allocation ruby_value_on_heap
    take(Some(Box::new(v))); //~ ruby_value_on_heap
    take(Box::new(move || v)); //~ ruby_value_on_heap
    let _outer = vec![vec![v]]; //~ infallible_allocation ruby_value_on_heap
    take(&[v]);
    take(kept.len());
    take(Box::new(|| kept.len()));
    take(kept);
}

thread_local! {
    static LIST: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) }; //~ ruby_value_on_heap
    static ONE: Cell<Option<RString>> = const { Cell::new(None) }; //~ ruby_value_on_heap
    static COUNT: Cell<u32> = const { Cell::new(0) };
}

/* ---- the boundary lints (`lints/makiri_lints/src/boundaries.rs`) ---- */

/// Stands in for an engine module: `ruby_layer_in_engine` treats
/// `lint_probe::engine` as one.
#[rustfmt::skip] // keeps each marker on the line reported
mod engine {
    pub fn names_the_ruby_layer() -> crate::bridge::rb::Error { //~ ruby_layer_in_engine
        crate::bridge::ruby::type_error("x") //~ ruby_layer_in_engine
    }

    pub fn names_only_the_engine(b: &[u8]) -> bool {
        crate::utf8_decode::valid(b)
    }
}

extern "C" fn unguarded() {} //~ unguarded_no_unwind_site

extern "C" fn guarded() {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::SelfTest, _>(|| ())
}

struct Dropped;

impl Drop for Dropped {
    fn drop(&mut self) {} //~ unguarded_no_unwind_site
}

struct GuardedDrop;

impl Drop for GuardedDrop {
    fn drop(&mut self) {
        crate::no_unwind::must_not_unwind::<crate::no_unwind::SelfTest, _>(|| ())
    }
}

fn gvl_released(ruby: &crate::bridge::rb::Ruby) -> Result<(), crate::bridge::rb::Error> {
    let _ = crate::bridge::gvl::without_gvl(ruby, || crate::bridge::ruby::type_error("x")); //~ discarded_ruby_error ruby_in_gvl_released_body
    crate::bridge::gvl::without_gvl(ruby, || engine::names_only_the_engine(b"x"))?;
    Ok(())
}

fn allocations() {
    let _ = "x".to_owned(); //~ infallible_allocation
    let _ = format!("{}", 1); //~ infallible_allocation
    let _ = core::format_args!("{}", 1);
}

fn callers_ruby(ruby: &crate::bridge::rb::Ruby, m: crate::bridge::rb::RModule) {
    let _ = crate::bridge::rb::Module::call_method(m, ruby, "new", &[]); //~ discarded_ruby_error runs_callers_ruby
    let _ = crate::bridge::rb::method::yield_value(ruby, ruby.qnil()); //~ discarded_ruby_error runs_callers_ruby
}

/// Stands in for a `bridge/` module: the bridge-scoped lints treat
/// `lint_probe::bridge` as one (and not as a home of raw `VALUE`s).
#[rustfmt::skip] // keeps each marker on the line reported
mod bridge {
    use crate::html_dom::html::{HtmlNode, RawNode};

    fn crossing(n: HtmlNode<'_>) -> RawNode {
        RawNode::from(n) //~ raw_node_crossing
    }

    fn crossing_by_path(n: Option<HtmlNode<'_>>) -> Option<RawNode> {
        n.map(RawNode::from) //~ raw_node_crossing
    }

    fn crossing_by_into(n: HtmlNode<'_>) -> RawNode {
        n.into() //~ raw_node_crossing
    }

    fn surface(m: crate::bridge::rb::RModule, f: crate::bridge::rb::method::Func) {
        let _ = crate::bridge::rb::Module::define_method(m, "x", f); //~ discarded_ruby_error ruby_surface_in_bridge
    }

    fn splits_arguments() -> crate::bridge::args::Kwargs {
        crate::bridge::args::Kwargs::from_scan(None) //~ ruby_surface_in_bridge
    }
}

fn fallible() -> Result<u32, crate::bridge::rb::Error> {
    Ok(1)
}

#[rustfmt::skip] // keeps each marker on the line reported
fn discards_ruby_errors() {
    let _ = fallible().ok(); //~ discarded_ruby_error
    let _ = fallible(); //~ discarded_ruby_error
    let _unread = fallible(); //~ discarded_ruby_error
    let _e = crate::bridge::ruby::type_error("x"); //~ discarded_ruby_error
    let _ = fallible().is_ok(); //~ discarded_ruby_error
    let _ = fallible().unwrap_or(0); //~ discarded_ruby_error
    let _n: Result<u32, ()> = fallible().or(Ok(0)); //~ discarded_ruby_error
    let _n = fallible().map_err(|_| crate::bridge::ruby::type_error("x")); //~ discarded_ruby_error
    let _n = fallible().or_else(|_e| fallible()); //~ discarded_ruby_error
    if let Ok(_n) = fallible() {} //~ discarded_ruby_error
    if let Err(_) = fallible() {} //~ discarded_ruby_error
    let Ok(_n) = fallible() else { return }; //~ discarded_ruby_error
    match fallible() {
        Ok(_n) => {}
        Err(_) => {} //~ discarded_ruby_error
    }
    match fallible() {
        Ok(_n) => {}
        _ => {} //~ discarded_ruby_error
    }
    if let Err(e) = fallible() {
        drop(e); //~ discarded_ruby_error
    }
    core::mem::forget(fallible()); //~ discarded_ruby_error
}

fn passes_ruby_errors_on(ruby: &crate::bridge::rb::Ruby) -> Result<u32, crate::bridge::rb::Error> {
    let n = fallible()?;
    let _ = fallible().map(|_| ())?;
    if let Err(e) = fallible() {
        return Err(e);
    }
    /* Handled by naming what is handled; an exit comes back as `Err`. */
    match fallible() {
        Ok(_n) => {}
        Err(e) => {
            e.rescue(ruby, &[crate::bridge::rb::Builtin::ArgumentError])?;
        }
    }
    let _ = fallible().map_err(|e| e)?;
    Ok(n)
}
