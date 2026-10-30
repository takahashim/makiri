//! What the crate's own lint (`lints/makiri_lints`: `ruby_value_on_heap`)
//! must report, and what it must leave alone. Compiled only under
//! `--cfg makiri_lint_probe`, by `rake dylint`, which fails unless the lint
//! fires on exactly the lines that end in `//~` - so a lint that has stopped
//! seeing a case cannot pass.
#![allow(dead_code, clippy::all)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::bridge::rb::Opaque;
use crate::bridge::rb::{RString, Value};

use crate::bridge::typed::Payload;

struct Kept {
    values: Vec<(Value, u32)>, //~
    marked: Opaque<Value>,
    borrowed: &'static [Value],
}

struct Data {
    value: Value, //~
}

impl Payload for Data {}

type Strings = Vec<RString>;

fn parameter(_: Strings) {} //~

#[rustfmt::skip] // keeps the marker on the line reported
fn returned(v: Value) -> Box<Value> { //~
    Box::new(v)
}

fn bindings(v: Value) {
    let _map: HashMap<u32, Value> = HashMap::new(); //~
    let _kept = vec![v]; //~
    let _on_stack = v;
    let _slice: &[Value] = &[];
}

fn take<T>(_: T) {}

/// Temporaries no binding names, which only the expression check sees.
#[rustfmt::skip] // keeps each marker on the line reported
fn temporaries(v: Value, kept: &Vec<Value>) {
    take(vec![v]); //~
    let _n = vec![v].len(); //~
    take(Some(Box::new(v))); //~
    take(Box::new(move || v)); //~
    let _outer = vec![vec![v]]; //~
    take(&[v]);
    take(kept.len());
    take(Box::new(|| kept.len()));
    take(kept);
}

thread_local! {
    static LIST: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) }; //~
    static ONE: Cell<Option<RString>> = const { Cell::new(None) }; //~
    static COUNT: Cell<u32> = const { Cell::new(0) };
}

/* ---- the boundary lints (`lints/makiri_lints/src/boundaries.rs`) ---- */

/// Stands in for an engine module: `ruby_layer_in_engine` treats
/// `lint_probe::engine` as one.
#[rustfmt::skip] // keeps each marker on the line reported
mod engine {
    pub fn names_the_ruby_layer() -> crate::bridge::rb::Error { //~
        crate::bridge::ruby::type_error("x") //~
    }

    pub fn names_only_the_engine(b: &[u8]) -> bool {
        crate::cutf8::valid(b)
    }
}

extern "C" fn unguarded() {} //~

extern "C" fn guarded() {
    crate::no_unwind::must_not_unwind::<crate::no_unwind::SelfTest, _>(|| ())
}

struct Dropped;

impl Drop for Dropped {
    fn drop(&mut self) {} //~
}

struct GuardedDrop;

impl Drop for GuardedDrop {
    fn drop(&mut self) {
        crate::no_unwind::must_not_unwind::<crate::no_unwind::SelfTest, _>(|| ())
    }
}

fn gvl_released(ruby: &crate::bridge::rb::Ruby) -> Result<(), crate::bridge::rb::Error> {
    let _ = crate::bridge::gvl::without_gvl(ruby, || crate::bridge::ruby::type_error("x")); //~
    crate::bridge::gvl::without_gvl(ruby, || engine::names_only_the_engine(b"x"))?;
    Ok(())
}

fn allocations() {
    let _ = "x".to_owned(); //~
    let _ = format!("{}", 1); //~
    let _ = core::format_args!("{}", 1);
}

fn callers_ruby(ruby: &crate::bridge::rb::Ruby, m: crate::bridge::rb::RModule) {
    let _ = crate::bridge::rb::Module::call_method(m, ruby, "new", &[]); //~
    let _ = crate::bridge::rb::method::yield_value(ruby, ruby.qnil()); //~
}

/// Stands in for a `bridge/` module: the bridge-scoped lints treat
/// `lint_probe::bridge` as one (and not as a home of raw `VALUE`s).
#[rustfmt::skip] // keeps each marker on the line reported
mod bridge {
    use crate::html_dom::html::{HtmlNode, RawNode};

    fn crossing(n: HtmlNode<'_>) -> RawNode {
        RawNode::from(n) //~
    }

    fn crossing_by_path(n: Option<HtmlNode<'_>>) -> Option<RawNode> {
        n.map(RawNode::from) //~
    }

    fn crossing_by_into(n: HtmlNode<'_>) -> RawNode {
        n.into() //~
    }

    fn surface(m: crate::bridge::rb::RModule, f: crate::bridge::rb::method::Func) {
        let _ = crate::bridge::rb::Module::define_method(m, "x", f); //~
    }

    fn splits_arguments() -> crate::bridge::args::Kwargs {
        crate::bridge::args::Kwargs::from_scan(None) //~
    }
}

fn fallible() -> Result<u32, crate::bridge::rb::Error> {
    Ok(1)
}

#[rustfmt::skip] // keeps each marker on the line reported
fn discards_ruby_errors() {
    let _ = fallible().ok(); //~
    let _ = fallible(); //~
    let _unread = fallible(); //~
    let _e = crate::bridge::ruby::type_error("x"); //~
    let _ = fallible().is_ok(); //~
    let _ = fallible().unwrap_or(0); //~
    let _n: Result<u32, ()> = fallible().or(Ok(0)); //~
    let _n = fallible().map_err(|_| crate::bridge::ruby::type_error("x")); //~
    let _n = fallible().or_else(|_e| fallible()); //~
    if let Ok(_n) = fallible() {} //~
    if let Err(_) = fallible() {} //~
    let Ok(_n) = fallible() else { return }; //~
    match fallible() {
        Ok(_n) => {}
        Err(_) => {} //~
    }
    match fallible() {
        Ok(_n) => {}
        _ => {} //~
    }
    if let Err(e) = fallible() {
        drop(e); //~
    }
    core::mem::forget(fallible()); //~
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
