//! `Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array`: phase two of the
//! stylesheet binding, the owned Rust rules [`crate::lexbor::stylesheet::parse`]
//! returns turned into plain Ruby hashes and arrays.
//!
//! Phase one has dropped everything it built before this runs; what is left
//! owned - the parsed rules and the input's copy - is the caller's frame's,
//! and phase two runs under `protect`, so a raise from building Ruby objects
//! comes back as `Err` and those drop. See `lexbor::stylesheet`.

use magnus::{function, prelude::*, Error, RArray, RHash, Ruby, StaticSymbol, Value};

use crate::bridge::ruby::error_class;
use crate::bridge::string::{ruby_verified_text, utf8_str};
use crate::init::MOD_LEXBOR;
use crate::lexbor::stylesheet::{parse, Decl, Fail, Rule, MAX_DEPTH};

/// The fixed hash keys and `:type` values, interned once per call.
///
/// STATIC symbols (`rb_intern`), which are never collected. `to_symbol` made
/// dynamic ones, and re-interning one the GC had just freed - a large sheet
/// makes plenty of garbage between calls - took Ruby's resurrection path,
/// which Valgrind reports as a read of uninitialised GC state.
///
/// Interned once so the conversion loops do not hash-look-up every key again
/// for every declaration; a per-call struct does that without process-global
/// mutable state, and a stylesheet is one call.
struct Keys {
    type_: StaticSymbol,
    selectors: StaticSymbol,
    declarations: StaticSymbol,
    name: StaticSymbol,
    value: StaticSymbol,
    important: StaticSymbol,
    text: StaticSymbol,
    specificity: StaticSymbol,
    selector_text: StaticSymbol,
    prelude: StaticSymbol,
    rules: StaticSymbol,
    sym_style: StaticSymbol,
    sym_bad_style: StaticSymbol,
    sym_at_rule: StaticSymbol,
}

impl Keys {
    fn new(ruby: &Ruby) -> Keys {
        Keys {
            type_: ruby.sym_new("type"),
            selectors: ruby.sym_new("selectors"),
            declarations: ruby.sym_new("declarations"),
            name: ruby.sym_new("name"),
            value: ruby.sym_new("value"),
            important: ruby.sym_new("important"),
            text: ruby.sym_new("text"),
            specificity: ruby.sym_new("specificity"),
            selector_text: ruby.sym_new("selector_text"),
            prelude: ruby.sym_new("prelude"),
            rules: ruby.sym_new("rules"),
            sym_style: ruby.sym_new("style"),
            sym_bad_style: ruby.sym_new("bad_style"),
            sym_at_rule: ruby.sym_new("at_rule"),
        }
    }
}

fn decls_to_ruby(ruby: &Ruby, k: &Keys, ds: &[Decl]) -> Result<RArray, Error> {
    let a = ruby.ary_new_capa(ds.len());
    for d in ds {
        let h = ruby.hash_new();
        h.aset(k.name, utf8_str(&d.name))?;
        h.aset(k.value, utf8_str(&d.value))?;
        h.aset(k.important, d.important)?;
        a.push(h)?;
    }
    Ok(a)
}

fn rules_to_ruby(ruby: &Ruby, k: &Keys, rs: &[Rule]) -> Result<RArray, Error> {
    let a = ruby.ary_new_capa(rs.len());
    for r in rs {
        let h: RHash = ruby.hash_new();
        match r {
            Rule::Style {
                selectors,
                declarations,
            } => {
                h.aset(k.type_, k.sym_style)?;
                let sa = ruby.ary_new_capa(selectors.len());
                for s in selectors {
                    let sh = ruby.hash_new();
                    sh.aset(k.text, utf8_str(&s.text))?;
                    let sp = ruby.ary_new_capa(3);
                    for v in s.specificity {
                        sp.push(v as i64)?;
                    }
                    sh.aset(k.specificity, sp)?;
                    sa.push(sh)?;
                }
                h.aset(k.selectors, sa)?;
                h.aset(k.declarations, decls_to_ruby(ruby, k, declarations)?)?;
            }
            Rule::BadStyle {
                selector_text,
                declarations,
            } => {
                h.aset(k.type_, k.sym_bad_style)?;
                h.aset(k.selector_text, utf8_str(selector_text))?;
                h.aset(k.declarations, decls_to_ruby(ruby, k, declarations)?)?;
            }
            Rule::At {
                name,
                prelude,
                rules,
                declarations,
            } => {
                h.aset(k.type_, k.sym_at_rule)?;
                h.aset(k.name, utf8_str(name))?;
                h.aset(k.prelude, utf8_str(prelude))?;
                h.aset(k.rules, rules_to_ruby(ruby, k, rules)?)?;
                h.aset(k.declarations, decls_to_ruby(ruby, k, declarations)?)?;
            }
        }
        a.push(h)?;
    }
    Ok(a)
}

/// Parses untrusted input, so it runs under `bridge::ruby::entry`: a panic is
/// `Makiri::InternalError`, not `fatal`.
fn parse_stylesheet(ruby: &Ruby, text: Value) -> Result<RArray, Error> {
    crate::bridge::ruby::entry(|| parse_stylesheet_inner(ruby, text))
}

fn parse_stylesheet_inner(ruby: &Ruby, text: Value) -> Result<RArray, Error> {
    let tv = ruby_verified_text(text, "CSS stylesheet")?;
    /* `tv` is this frame's own copy of the String's bytes, so nothing Ruby
     * does can move or mutate them. */
    let css: &[u8] = tv.as_bytes();

    let eclass = error_class();
    let err = |m: &str| Error::new(eclass, m.to_owned());

    let parsed = match parse(css) {
        Ok(parsed) => parsed,
        Err(Fail::Oom) => return Err(err("out of memory parsing CSS stylesheet")),
        Err(Fail::TooDeep) => {
            return Err(Error::new(
                eclass,
                format!("CSS at-rule nesting too deep (max {MAX_DEPTH})"),
            ))
        }
        Err(Fail::Parse) => return Err(err("failed to parse CSS stylesheet")),
    };

    // ---- phase two ----
    /* Under protect: building the result allocates Ruby objects, and a raise
     * from the allocator would unwind past this frame, which owns the parsed
     * rules and the input's copy. */
    crate::bridge::ruby::protected(|| rules_to_ruby(ruby, &Keys::new(ruby), &parsed))
}

/// `Makiri::Lexbor::CSS.parse_stylesheet`. From `Init_makiri`.
pub fn init_lexbor_css(ruby: &Ruby) -> Result<(), Error> {
    let lexbor = MOD_LEXBOR.defined()?;
    let css = ruby.module_new();
    lexbor.const_set("CSS", css)?;
    css.define_module_function("parse_stylesheet", function!(parse_stylesheet, 1))?;
    Ok(())
}
