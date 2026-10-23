//! `Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array`: phase two of the
//! stylesheet binding, the owned Rust rules [`crate::selectors::stylesheet::parse`]
//! returns turned into plain Ruby hashes and arrays.
//!
//! Phase one has dropped everything it built before this runs; what is left
//! owned - the parsed rules and the input's copy - is the caller's frame's,
//! and phase two builds through `bridge::build`, so a failed allocation comes
//! back as `Err` and those drop. See `selectors::stylesheet`.

use magnus::{function, prelude::*, Error, RArray, RHash, Ruby, Value};

use crate::bridge::build::{ary, aset, hash, int, push, sym};
use crate::bridge::ruby::{boolean, error_class};
use crate::bridge::string::{ruby_verified_text, utf8_str};
use crate::init::MOD_LEXBOR;
use crate::selectors::stylesheet::{parse, Decl, Fail, Rule, MAX_DEPTH};

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
    type_: Value,
    selectors: Value,
    declarations: Value,
    name: Value,
    value: Value,
    important: Value,
    text: Value,
    specificity: Value,
    selector_text: Value,
    prelude: Value,
    rules: Value,
    sym_style: Value,
    sym_bad_style: Value,
    sym_at_rule: Value,
}

impl Keys {
    fn new() -> Result<Keys, Error> {
        Ok(Keys {
            type_: sym("type")?,
            selectors: sym("selectors")?,
            declarations: sym("declarations")?,
            name: sym("name")?,
            value: sym("value")?,
            important: sym("important")?,
            text: sym("text")?,
            specificity: sym("specificity")?,
            selector_text: sym("selector_text")?,
            prelude: sym("prelude")?,
            rules: sym("rules")?,
            sym_style: sym("style")?,
            sym_bad_style: sym("bad_style")?,
            sym_at_rule: sym("at_rule")?,
        })
    }
}

fn decls_to_ruby(k: &Keys, ds: &[Decl]) -> Result<RArray, Error> {
    let a = ary(ds.len())?;
    for d in ds {
        let h = hash()?;
        aset(h, k.name, utf8_str(&d.name)?)?;
        aset(h, k.value, utf8_str(&d.value)?)?;
        aset(h, k.important, boolean(d.important))?;
        push(a, h.as_value())?;
    }
    Ok(a)
}

fn rules_to_ruby(k: &Keys, rs: &[Rule]) -> Result<RArray, Error> {
    let a = ary(rs.len())?;
    for r in rs {
        let h: RHash = hash()?;
        match r {
            Rule::Style {
                selectors,
                declarations,
            } => {
                aset(h, k.type_, k.sym_style)?;
                let sa = ary(selectors.len())?;
                for s in selectors {
                    let sh = hash()?;
                    aset(sh, k.text, utf8_str(&s.text)?)?;
                    let sp = ary(3)?;
                    for v in s.specificity {
                        push(sp, int(v as i64)?)?;
                    }
                    aset(sh, k.specificity, sp.as_value())?;
                    push(sa, sh.as_value())?;
                }
                aset(h, k.selectors, sa.as_value())?;
                aset(
                    h,
                    k.declarations,
                    decls_to_ruby(k, declarations)?.as_value(),
                )?;
            }
            Rule::BadStyle {
                selector_text,
                declarations,
            } => {
                aset(h, k.type_, k.sym_bad_style)?;
                aset(h, k.selector_text, utf8_str(selector_text)?)?;
                aset(
                    h,
                    k.declarations,
                    decls_to_ruby(k, declarations)?.as_value(),
                )?;
            }
            Rule::At {
                name,
                prelude,
                rules,
                declarations,
            } => {
                aset(h, k.type_, k.sym_at_rule)?;
                aset(h, k.name, utf8_str(name)?)?;
                aset(h, k.prelude, utf8_str(prelude)?)?;
                aset(h, k.rules, rules_to_ruby(k, rules)?.as_value())?;
                aset(
                    h,
                    k.declarations,
                    decls_to_ruby(k, declarations)?.as_value(),
                )?;
            }
        }
        push(a, h.as_value())?;
    }
    Ok(a)
}

/// Parses untrusted input, so it runs under `bridge::ruby::entry`: a panic is
/// `Makiri::InternalError`, not `fatal`.
fn parse_stylesheet(ruby: &Ruby, text: Value) -> Result<RArray, Error> {
    crate::bridge::ruby::entry(|| parse_stylesheet_inner(ruby, text))
}

fn parse_stylesheet_inner(_ruby: &Ruby, text: Value) -> Result<RArray, Error> {
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
    /* Building the result allocates Ruby objects; a failed allocation comes
     * back as `Err` (`bridge::build`), and the parsed rules and the input's
     * copy drop with this frame. */
    rules_to_ruby(&Keys::new()?, &parsed)
}

/// `Makiri::Lexbor::CSS.parse_stylesheet`. From `Init_makiri`.
pub fn init_lexbor_css(ruby: &Ruby) -> Result<(), Error> {
    let lexbor = MOD_LEXBOR.defined()?;
    let css = ruby.module_new();
    lexbor.const_set("CSS", css)?;
    css.define_module_function("parse_stylesheet", function!(parse_stylesheet, 1))?;
    Ok(())
}
