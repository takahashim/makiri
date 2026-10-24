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

use crate::bridge::typed::{Hooks, Tracer};

struct Kept {
    values: Vec<(Value, u32)>, //~
    marked: Opaque<Value>,
    borrowed: &'static [Value],
}

struct Data {
    value: Value, //~
}

impl Hooks for Data {
    fn trace(&self, _: &Tracer) {}
}

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

thread_local! {
    static LIST: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) }; //~
    static ONE: Cell<Option<RString>> = const { Cell::new(None) }; //~
    static COUNT: Cell<u32> = const { Cell::new(0) };
}
