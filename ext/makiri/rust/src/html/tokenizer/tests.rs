//! The tokenizer read into the conformance harness's token model
//! (`crate::conformance::tokens`), as `lexbor::conformance_tests` reads
//! Lexbor's, and checked against the html5lib tokenizer tests.

#![allow(clippy::disallowed_methods)]

use super::{
    cdata_section_before, data_before, flags, plaintext_before, rawtext_before, rcdata_before,
    script_data_before, Status, Stop, TokenSink, Tokenizer,
};
use crate::conformance::tokens::{self as tok, State, Token, TokenizerTest};
use crate::html::names::Names;
use crate::html::res::names as res;

/// Collects every token, read the way Lexbor's are read, with where it
/// began and ended.
struct Collect(Vec<Token>, Vec<(Option<usize>, Option<usize>)>, Names);

impl TokenSink for Collect {
    fn token(&mut self, t: &mut Tokenizer<'_, '_>) -> Result<(), Stop> {
        if let Some(tk) = read(&self.2, t) {
            self.0.push(tk);
            self.1.push((t.token.begin, t.token.end));
        }
        Ok(())
    }

    fn names(&mut self) -> &mut Names {
        &mut self.2
    }
}

/// What one run produced, for an exact comparison with Lexbor: the tokens,
/// each one's `(begin, end)`, and the parse errors as `(id, position)`.
#[derive(Debug, PartialEq, Eq)]
pub struct Trace {
    pub tokens: Vec<Token>,
    pub positions: Vec<(Option<usize>, Option<usize>)>,
    pub errors: Vec<(u32, Option<usize>)>,
}

fn read(names: &Names, t: &mut Tokenizer<'_, '_>) -> Option<Token> {
    let tag = t.token.tag_id;
    let text = t.text().to_vec();
    match tag {
        res::TAG__END_OF_FILE => None,
        res::TAG__TEXT => Some(Token::Character(text)),
        res::TAG__EM_COMMENT => Some(Token::Comment(text)),
        res::TAG__PROCESSINGINSTRUCTION => {
            /* As the states store it - the target's length in
             * `pi_target_len` - not through `Tokenizer::pi_parts`, the tree
             * builder's reading, for the reason the DOCTYPE case below
             * gives. */
            let at = t.token.pi_target_len.min(text.len());
            Some(Token::Pi {
                target: text[..at].to_vec(),
                data: text[at..].to_vec(),
            })
        }
        res::TAG__EM_DOCTYPE => {
            /* Read the attributes as the states store them (`state_doctype.rs`),
             * not through `Tokenizer::doctype`: that is the tree builder's
             * reading, which the tree-construction tests check; this suite
             * checks what the states store. */
            let attrs = t.token.attrs.clone();
            let attr_name = |id: Option<usize>| {
                id.and_then(|id| names.attr_name(id).map(<[u8]>::to_vec))
                    .unwrap_or_default()
            };
            let value =
                |t: &Tokenizer<'_, '_>, a: &super::TokenAttr| t.attr_value(a).map(<[u8]>::to_vec);
            let name = attrs
                .first()
                .filter(|a| a.name_begin.is_some())
                .map(|a| attr_name(a.name));
            let (mut public_id, mut system_id) = (None, None);
            if let Some(second) = attrs.get(1) {
                if second.name == Some(res::ATTR_PUBLIC) {
                    public_id = value(t, second);
                    if let Some(third) = attrs.get(2) {
                        system_id = value(t, third);
                    }
                } else if second.name == Some(res::ATTR_SYSTEM) {
                    system_id = value(t, second);
                }
            }
            Some(Token::Doctype {
                name,
                public_id,
                system_id,
                force_quirks: t.token.flags & flags::FORCE_QUIRKS != 0,
            })
        }
        _ => {
            let name = names.tag_name(tag).map(<[u8]>::to_vec).unwrap_or_default();
            if t.token.flags & flags::CLOSE != 0 {
                return Some(Token::EndTag { name });
            }
            let attrs = t.token.attrs.clone();
            let attrs = attrs
                .iter()
                .map(|a| {
                    let n = a
                        .name
                        .and_then(|id| names.attr_name(id).map(<[u8]>::to_vec));
                    (
                        n.unwrap_or_default(),
                        t.attr_value(a).map(<[u8]>::to_vec).unwrap_or_default(),
                    )
                })
                .collect();
            Some(Token::StartTag {
                name,
                attrs,
                self_closing: t.token.flags & flags::CLOSE_SELF != 0,
            })
        }
    }
}

/// Tokenize `input` alone from `state`, as the html5lib tests set it up.
pub fn tokenize(input: &[u8], state: State, last_start_tag: Option<&[u8]>) -> Vec<Token> {
    tokenize_traced(input, state, last_start_tag).tokens
}

/// [`tokenize`], with positions and parse errors.
pub fn tokenize_traced(input: &[u8], state: State, last_start_tag: Option<&[u8]>) -> Trace {
    let mut names = Names::default();
    let last = last_start_tag.map(|n| names.tag_append_lower(n).expect("interns"));
    let mut sink = Collect(Vec::new(), Vec::new(), names);
    let errors;
    {
        let mut t = Tokenizer::new(input, &mut sink);
        let (f, wants_last): (super::State, bool) = match state {
            State::Data => (data_before, false),
            State::Plaintext => (plaintext_before, false),
            State::Rcdata => (rcdata_before, true),
            State::Rawtext => (rawtext_before, true),
            State::ScriptData => (script_data_before, true),
            State::CdataSection => (cdata_section_before, false),
        };
        t.set_state(f);
        if let (true, Some(id)) = (wants_last, last) {
            t.tmp_tag_id = id;
        }
        assert_eq!(t.run(), Status::Ok);
        errors = t.errors().iter().map(|e| (e.id as u32, e.pos)).collect();
    }
    Trace {
        tokens: sink.0,
        positions: sink.1,
        errors,
    }
}

#[test]
fn tokenizes_from_each_initial_state() {
    let toks = tokenize(
        b"<a href='x&amp;y' b>t</a><!--c--><?p q?>",
        State::Data,
        None,
    );
    assert_eq!(
        toks,
        vec![
            Token::StartTag {
                name: b"a".to_vec(),
                attrs: vec![(b"href".to_vec(), b"x&y".to_vec()), (b"b".to_vec(), vec![])],
                self_closing: false
            },
            Token::Character(b"t".to_vec()),
            Token::EndTag {
                name: b"a".to_vec()
            },
            Token::Comment(b"c".to_vec()),
            Token::Pi {
                target: b"p".to_vec(),
                data: b"q".to_vec()
            },
        ]
    );
}

/// Every html5lib tokenizer test through the port.
#[test]
#[ignore = "needs the fetched html5lib-tests data; run by `rake conformance:parser`"]
fn html5lib_tokenizer() {
    let dir = crate::conformance::tokenizer_dir();
    let tests: Vec<TokenizerTest> = tok::read_dir(&dir).expect("html5lib tokenizer tests");
    let (mut pass, mut skipped) = (0, 0);
    let mut failures = Vec::new();
    for t in &tests {
        if t.unrepresentable {
            skipped += 1;
            continue;
        }
        let mut bad = Vec::new();
        for &state in &t.states {
            let actual = tokenize(&t.input, state, t.last_start_tag.as_deref());
            if !tok::html5lib_eq(&actual, &t.output) {
                bad.push(format!(
                    "  {state:?}:\n    expected {:?}\n    actual   {:?}",
                    tok::coalesce(&t.output),
                    tok::coalesce(&actual)
                ));
            }
        }
        if bad.is_empty() {
            pass += 1;
        } else {
            failures.push(format!(
                "{} {:?}\n  input {:?}\n{}",
                t.key(),
                t.description,
                String::from_utf8_lossy(&t.input),
                bad.join("\n")
            ));
        }
    }
    eprintln!(
        "html5lib tokenizer through the port: {} tests, {pass} pass, {skipped} unrepresentable skipped, {} fail",
        tests.len(),
        failures.len()
    );
    let shown: Vec<_> = failures.iter().take(15).cloned().collect();
    assert!(
        failures.is_empty(),
        "{} failing:\n{}",
        failures.len(),
        shown.join("\n\n")
    );
}
