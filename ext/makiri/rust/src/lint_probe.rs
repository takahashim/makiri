//! What the crate's own lints (`lints/makiri_lints`: `ruby_value_on_heap`,
//! `converted_argument`) must report, and what they must leave alone.
//! Compiled only under `--cfg makiri_lint_probe`, by `rake dylint`, which
//! fails unless the lints fire on exactly the lines that end in `//~` - so a
//! lint that has stopped seeing a case cannot pass.
#![allow(dead_code, clippy::all)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use magnus::value::Opaque;
use magnus::{RString, Value};

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

/* `converted_argument`: what a registered function may take. */

fn takes_string(_: Value, _: RString) -> Value {
    unimplemented!()
}
fn takes_int(_: i64) -> Value {
    unimplemented!()
}
fn takes_hash(_: &magnus::Ruby, _: Value, _: Option<magnus::RHash>) -> Value {
    unimplemented!()
}
fn string_receiver(_: RString) -> Value {
    unimplemented!()
}
fn takes_values(_: &magnus::Ruby, _: Value, _: Value, _: Option<Value>, _: bool) -> Value {
    unimplemented!()
}
fn takes_rest(_: Value, _: &[Value]) -> Value {
    unimplemented!()
}

fn registrations(class: magnus::RClass) -> Result<(), magnus::Error> {
    use magnus::{function, method, Module, Object};
    class.define_method("a", method!(takes_string, 1))?; //~
    class.define_singleton_method("b", function!(takes_int, 1))?; //~
    class.define_method("c", method!(takes_hash, 1))?; //~
    class.define_method("d", method!(string_receiver, 0))?; //~
    class.define_method("e", method!(takes_values, 3))?;
    class.define_method("f", method!(takes_rest, -1))?;
    Ok(())
}
