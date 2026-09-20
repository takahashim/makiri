//! `Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array`: phase two of the
//! stylesheet binding, the owned Rust rules [`crate::lexbor::stylesheet::parse`]
//! returns turned into plain Ruby hashes and arrays.
//!
//! Phase one has dropped every Lexbor resource before this runs, so building
//! Ruby objects - each one a possible raise - leaves nothing to free. That is
//! the whole reason for the two phases; see `lexbor::stylesheet`.

use magnus::{function, prelude::*, Error, RArray, RHash, Ruby, Symbol, Value};

use crate::bridge::ruby::error_class;
use crate::bridge::string::ruby_verified_text;
use crate::init::MOD_LEXBOR;
use crate::lexbor::stylesheet::{parse, Decl, Fail, Rule, MAX_DEPTH};

/// The fixed hash keys and `:type` values, interned once per call.
///
/// Interned once so the conversion loops do not hash-look-up every key again
/// for every declaration; a per-call struct does that without process-global
/// mutable state, and a stylesheet is one call.
struct Keys {
    type_: Symbol,
    selectors: Symbol,
    declarations: Symbol,
    name: Symbol,
    value: Symbol,
    important: Symbol,
    text: Symbol,
    specificity: Symbol,
    selector_text: Symbol,
    prelude: Symbol,
    rules: Symbol,
    sym_style: Symbol,
    sym_bad_style: Symbol,
    sym_at_rule: Symbol,
}

impl Keys {
    fn new(ruby: &Ruby) -> Keys {
        Keys {
            type_: ruby.to_symbol("type"),
            selectors: ruby.to_symbol("selectors"),
            declarations: ruby.to_symbol("declarations"),
            name: ruby.to_symbol("name"),
            value: ruby.to_symbol("value"),
            important: ruby.to_symbol("important"),
            text: ruby.to_symbol("text"),
            specificity: ruby.to_symbol("specificity"),
            selector_text: ruby.to_symbol("selector_text"),
            prelude: ruby.to_symbol("prelude"),
            rules: ruby.to_symbol("rules"),
            sym_style: ruby.to_symbol("style"),
            sym_bad_style: ruby.to_symbol("bad_style"),
            sym_at_rule: ruby.to_symbol("at_rule"),
        }
    }
}

/// Lexbor emits UTF-8 and the input was verified as UTF-8, so the String is
/// tagged UTF-8 rather than built as binary and re-tagged - which is what
/// `str_from_slice` would give, and what a spec caught.
fn str_of(ruby: &Ruby, b: &[u8]) -> Value {
    // `utf8_encoding` hands back CRuby's cached global, so this is not worth
    // threading through the conversion.
    ruby.enc_str_new(b, ruby.utf8_encoding()).as_value()
}

fn decls_to_ruby(ruby: &Ruby, k: &Keys, ds: &[Decl]) -> Result<RArray, Error> {
    let a = ruby.ary_new_capa(ds.len());
    for d in ds {
        let h = ruby.hash_new();
        h.aset(k.name, str_of(ruby, &d.name))?;
        h.aset(k.value, str_of(ruby, &d.value))?;
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
                    sh.aset(k.text, str_of(ruby, &s.text))?;
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
                h.aset(k.selector_text, str_of(ruby, selector_text))?;
                h.aset(k.declarations, decls_to_ruby(ruby, k, declarations)?)?;
            }
            Rule::At {
                name,
                prelude,
                rules,
            } => {
                h.aset(k.type_, k.sym_at_rule)?;
                h.aset(k.name, str_of(ruby, name))?;
                h.aset(k.prelude, str_of(ruby, prelude))?;
                h.aset(k.rules, rules_to_ruby(ruby, k, rules)?)?;
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
    let tv = ruby_verified_text(text, c"CSS stylesheet")?;
    /* `tv` anchors the String for this frame, and `parse` is Lexbor and the
     * allocator only - no Ruby runs that could move or mutate the bytes. */
    let css: &[u8] = tv.as_verified().as_bytes();

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
        Err(Fail::Init) => return Err(err("failed to initialise CSS parser")),
        Err(Fail::Parse) => return Err(err("failed to parse CSS stylesheet")),
        Err(Fail::Serialize) => return Err(err("failed to serialize CSS")),
    };

    // ---- phase two ----
    rules_to_ruby(ruby, &Keys::new(ruby), &parsed)
}

/// `Makiri::Lexbor::CSS.parse_stylesheet`. From `Init_makiri`.
pub fn init_lexbor_css() {
    let ruby = Ruby::get().expect("init_lexbor_css runs on the Ruby thread");
    let lexbor = magnus::RModule::from_value(MOD_LEXBOR.value())
        .expect("Makiri::Lexbor is a module by the time this runs");
    let css = ruby.module_new();
    lexbor.const_set("CSS", css).expect("Makiri::Lexbor::CSS");
    css.define_module_function("parse_stylesheet", function!(parse_stylesheet, 1))
        .expect("Makiri::Lexbor::CSS.parse_stylesheet");
}
