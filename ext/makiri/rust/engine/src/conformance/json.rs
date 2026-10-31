//! A small JSON reader for the html5lib tokenizer tests.
//!
//! The crate takes no dependencies beyond the Ruby binding, and the tests need
//! only JSON's data model. Strings decode to bytes: the tests write lone
//! surrogates (`"\uDFFF"`), which UTF-8 cannot hold, so a string records
//! whether it held one and the runner skips such a test - the html5lib README
//! says those are for UTF-16 implementations.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(JStr),
    Arr(Vec<Json>),
    /// In document order; keys may repeat.
    Obj(Vec<(String, Json)>),
}

/// A decoded string: UTF-8 bytes, with every lone surrogate encoded as its
/// WTF-8 three-byte form and `lone_surrogate` set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JStr {
    pub bytes: Vec<u8>,
    pub lone_surrogate: bool,
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&JStr> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("trailing data"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at byte {}", self.i)
    }
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected {:?}", c as char)))
        }
    }
    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("bad literal"))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    let v = self.value()?;
                    m.push((String::from_utf8_lossy(&k.bytes).into_owned(), v));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(self.err("expected , or }")),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(self.err("expected , or ]")),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(_) => {
                let start = self.i;
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i])
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .map(Json::Num)
                    .ok_or_else(|| self.err("bad number"))
            }
            None => Err(self.err("unexpected end")),
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("short \\u"))?;
        let v = std::str::from_utf8(h)
            .ok()
            .and_then(|s| u32::from_str_radix(s, 16).ok())
            .ok_or_else(|| self.err("bad \\u"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<JStr, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(self.err("expected string"));
        }
        self.i += 1;
        let mut out = JStr::default();
        loop {
            let c = *self
                .b
                .get(self.i)
                .ok_or_else(|| self.err("unterminated string"))?;
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' | b'\\' | b'/' => out.bytes.push(e),
                        b'b' => out.bytes.push(8),
                        b'f' => out.bytes.push(12),
                        b'n' => out.bytes.push(b'\n'),
                        b'r' => out.bytes.push(b'\r'),
                        b't' => out.bytes.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            push_cp(&mut out, cp);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                _ => out.bytes.push(c),
            }
        }
    }
}

/// Append `cp` as UTF-8, or a lone surrogate in its WTF-8 form.
pub fn push_cp(out: &mut JStr, cp: u32) {
    if (0xD800..0xE000).contains(&cp) {
        out.lone_surrogate = true;
        out.bytes.extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
        return;
    }
    let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
    let mut buf = [0u8; 4];
    out.bytes
        .extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shapes_the_tests_use() {
        let v = parse(
            r#"{"tests": [{"input": "aé😀\u0000", "x": [null, true, 1.5], "lone": "\udfff"}]}"#,
        )
        .expect("parses");
        let t = &v.get("tests").and_then(Json::as_arr).expect("tests")[0];
        let input = t.get("input").and_then(Json::as_str).expect("input");
        assert_eq!(input.bytes, "aé😀\0".as_bytes());
        assert!(!input.lone_surrogate);
        assert_eq!(
            t.get("x"),
            Some(&Json::Arr(vec![
                Json::Null,
                Json::Bool(true),
                Json::Num(1.5)
            ]))
        );
        assert!(
            t.get("lone")
                .and_then(Json::as_str)
                .expect("lone")
                .lone_surrogate
        );
    }
}
