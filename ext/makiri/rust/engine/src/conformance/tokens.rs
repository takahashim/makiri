//! The html5lib tokenizer tests: their token model and the `.test` reader.
//!
//! A tokenizer is read into [`Token`]s as a parser is read into a tree, so two
//! tokenizers compare with `==` (attributes in the order the token carries
//! them), and a tokenizer compares with the test data through
//! [`html5lib_eq`], which applies the format's own rules: adjacent character
//! tokens are one, and a start tag's attributes are a set.

use super::json::{self, JStr, Json};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Doctype {
        name: Option<Vec<u8>>,
        public_id: Option<Vec<u8>>,
        system_id: Option<Vec<u8>>,
        force_quirks: bool,
    },
    StartTag {
        name: Vec<u8>,
        attrs: Vec<(Vec<u8>, Vec<u8>)>,
        self_closing: bool,
    },
    EndTag {
        name: Vec<u8>,
    },
    Comment(Vec<u8>),
    Pi {
        target: Vec<u8>,
        data: Vec<u8>,
    },
    Character(Vec<u8>),
}

/// The tokenizer states a test can start in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Data,
    Plaintext,
    Rcdata,
    Rawtext,
    ScriptData,
    CdataSection,
}

impl State {
    fn from_name(n: &[u8]) -> Option<State> {
        Some(match n {
            b"Data state" => State::Data,
            b"PLAINTEXT state" => State::Plaintext,
            b"RCDATA state" => State::Rcdata,
            b"RAWTEXT state" => State::Rawtext,
            b"Script data state" => State::ScriptData,
            b"CDATA section state" => State::CdataSection,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TokenizerTest {
    pub file: String,
    /// 1-based position in its file.
    pub index: usize,
    pub description: String,
    pub input: Vec<u8>,
    pub output: Vec<Token>,
    pub states: Vec<State>,
    pub last_start_tag: Option<Vec<u8>>,
    /// The input or output holds a lone surrogate, which a UTF-8 tokenizer
    /// cannot be given; such a test is skipped, not run.
    pub unrepresentable: bool,
}

impl TokenizerTest {
    /// `test1.test#3`, the key the known divergence list uses.
    pub fn key(&self) -> String {
        format!("{}#{}", self.file, self.index)
    }
}

/// Merge adjacent character tokens, as the test format does.
pub fn coalesce(tokens: &[Token]) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    for t in tokens {
        match (out.last_mut(), t) {
            (Some(Token::Character(a)), Token::Character(b)) => a.extend_from_slice(b),
            (_, Token::Character(b)) if b.is_empty() => {}
            _ => out.push(t.clone()),
        }
    }
    out
}

fn normalized(tokens: &[Token]) -> Vec<Token> {
    coalesce(tokens)
        .into_iter()
        .map(|t| match t {
            Token::StartTag {
                name,
                mut attrs,
                self_closing,
            } => {
                attrs.sort();
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
            }
            t => t,
        })
        .collect()
}

/// Whether `actual` is what the test data `expected` says, by the format's
/// rules: character tokens coalesced, attributes compared as a set.
pub fn html5lib_eq(actual: &[Token], expected: &[Token]) -> bool {
    normalized(actual) == normalized(expected)
}

/// The test data's second round of unescaping (`doubleEscaped`): each
/// `\uHHHH` becomes that code point.
fn double_unescape(s: &JStr) -> JStr {
    let b = &s.bytes;
    let mut out = JStr {
        bytes: Vec::new(),
        lone_surrogate: s.lone_surrogate,
    };
    let hex = |i: usize| -> Option<u32> {
        let h = b.get(i + 2..i + 6)?;
        if b.get(i..i + 2)? != b"\\u" {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
    };
    let mut i = 0;
    while i < b.len() {
        match hex(i) {
            Some(hi) if (0xD800..0xDC00).contains(&hi) => match hex(i + 6) {
                Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                    json::push_cp(&mut out, 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
                    i += 12;
                }
                _ => {
                    json::push_cp(&mut out, hi);
                    i += 6;
                }
            },
            Some(cp) => {
                json::push_cp(&mut out, cp);
                i += 6;
            }
            None => {
                out.bytes.push(b[i]);
                i += 1;
            }
        }
    }
    out
}

struct Reader {
    double: bool,
    unrepresentable: bool,
}

impl Reader {
    fn string(&mut self, v: &Json) -> Option<Vec<u8>> {
        let s = v.as_str()?;
        let s = if self.double {
            double_unescape(s)
        } else {
            s.clone()
        };
        self.unrepresentable |= s.lone_surrogate;
        Some(s.bytes)
    }
    fn opt_string(&mut self, v: Option<&Json>) -> Option<Vec<u8>> {
        v.and_then(|v| self.string(v))
    }
    fn token(&mut self, v: &Json) -> Result<Option<Token>, String> {
        let a = v.as_arr().ok_or("a token is not an array")?;
        let kind = a
            .first()
            .and_then(Json::as_str)
            .ok_or("a token has no type")?;
        let s = |r: &mut Reader, i: usize| r.opt_string(a.get(i));
        Ok(Some(match kind.bytes.as_slice() {
            b"Character" => Token::Character(s(self, 1).unwrap_or_default()),
            b"Comment" => Token::Comment(s(self, 1).unwrap_or_default()),
            b"ProcessingInstruction" => Token::Pi {
                target: s(self, 1).unwrap_or_default(),
                data: s(self, 2).unwrap_or_default(),
            },
            b"EndTag" => Token::EndTag {
                name: s(self, 1).unwrap_or_default(),
            },
            b"StartTag" => {
                let mut attrs = Vec::new();
                if let Some(Json::Obj(m)) = a.get(2) {
                    for (k, v) in m {
                        let name = self.string(&Json::Str(JStr {
                            bytes: k.as_bytes().to_vec(),
                            lone_surrogate: false,
                        }));
                        attrs.push((name.unwrap_or_default(), self.string(v).unwrap_or_default()));
                    }
                }
                Token::StartTag {
                    name: s(self, 1).unwrap_or_default(),
                    attrs,
                    self_closing: a.get(3).and_then(Json::as_bool).unwrap_or(false),
                }
            }
            b"DOCTYPE" => Token::Doctype {
                name: s(self, 1),
                public_id: s(self, 2),
                system_id: s(self, 3),
                force_quirks: !a.get(4).and_then(Json::as_bool).unwrap_or(true),
            },
            /* The old format listed parse errors among the tokens. */
            b"ParseError" => return Ok(None),
            other => {
                return Err(format!(
                    "unknown token type {:?}",
                    String::from_utf8_lossy(other)
                ))
            }
        }))
    }
}

/// Every test in one `.test` file's `text`.
pub fn parse(file: &str, text: &str) -> Result<Vec<TokenizerTest>, String> {
    let root = json::parse(text).map_err(|e| format!("{file}: {e}"))?;
    /* `xmlViolationTests` expect the infoset coercion, not plain tokenizing. */
    let Some(tests) = root.get("tests").and_then(Json::as_arr) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (i, t) in tests.iter().enumerate() {
        let double = t
            .get("doubleEscaped")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let mut r = Reader {
            double,
            unrepresentable: false,
        };
        let input = r
            .opt_string(t.get("input"))
            .ok_or(format!("{file}#{}: no input", i + 1))?;
        let mut output = Vec::new();
        for v in t.get("output").and_then(Json::as_arr).unwrap_or_default() {
            if let Some(tok) = r.token(v).map_err(|e| format!("{file}#{}: {e}", i + 1))? {
                output.push(tok);
            }
        }
        let states = match t.get("initialStates").and_then(Json::as_arr) {
            Some(names) => names
                .iter()
                .map(|n| {
                    n.as_str()
                        .and_then(|s| State::from_name(&s.bytes))
                        .ok_or(format!("{file}#{}: unknown state", i + 1))
                })
                .collect::<Result<_, _>>()?,
            None => vec![State::Data],
        };
        let last_start_tag = r.opt_string(t.get("lastStartTag"));
        let description = t
            .get("description")
            .and_then(Json::as_str)
            .map(|s| String::from_utf8_lossy(&s.bytes).into_owned())
            .unwrap_or_default();
        out.push(TokenizerTest {
            file: file.to_string(),
            index: i + 1,
            description,
            input,
            output,
            states,
            last_start_tag,
            unrepresentable: r.unrepresentable,
        });
    }
    Ok(out)
}

/// Every test of every `.test` file directly in `dir`, files in name order.
/// `None` when the directory holds none (the data is fetched, not vendored).
pub fn read_dir(dir: &Path) -> Option<Vec<TokenizerTest>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "test"))
        .collect();
    files.sort();
    if files.is_empty() {
        return None;
    }
    let mut all = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).ok()?;
        let name = p.file_name()?.to_string_lossy().into_owned();
        all.extend(parse(&name, &text).unwrap_or_else(|e| panic!("{e}")));
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tokens_states_and_double_escapes() {
        let text = r#"{"tests": [
          {"description": "d", "input": "<a b='1' c>x",
           "output": [["StartTag", "a", {"b": "1", "c": ""}], ["Character", "x"]],
           "initialStates": ["Data state", "RCDATA state"], "lastStartTag": "a"},
          {"description": "e", "doubleEscaped": true, "input": "\\u0000\\uD83D\\uDE00",
           "output": [["Character", "\\u0000\\uD83D\\uDE00"], ["DOCTYPE", "html", null, null, false],
                      ["ProcessingInstruction", "x", "y"]]},
          {"description": "f", "doubleEscaped": true, "input": "\\uDFFF", "output": []}
        ]}"#;
        let t = parse("t.test", text).expect("parses");
        assert_eq!(t[0].states, vec![State::Data, State::Rcdata]);
        assert_eq!(t[0].last_start_tag.as_deref(), Some(&b"a"[..]));
        assert_eq!(t[1].input, "\0😀".as_bytes());
        assert_eq!(
            t[1].output[1],
            Token::Doctype {
                name: Some(b"html".to_vec()),
                public_id: None,
                system_id: None,
                force_quirks: true
            }
        );
        assert!(!t[1].unrepresentable);
        assert!(t[2].unrepresentable);
        let actual = [
            Token::StartTag {
                name: b"a".to_vec(),
                attrs: vec![(b"c".to_vec(), vec![]), (b"b".to_vec(), b"1".to_vec())],
                self_closing: false,
            },
            Token::Character(b"x".to_vec()),
            Token::Character(vec![]),
        ];
        assert!(html5lib_eq(&actual, &t[0].output));
    }
}
