//! The character encoding of a document parse: the HTML Standard's encoding
//! sniffing algorithm ("Determining the character encoding"), its "change the
//! encoding" on a `<meta>` found while the confidence is tentative, and the
//! Encoding Standard's decoders (`encoding_rs`) - so bytes reach the parser as
//! a browser would read them.
//!
//! The steps the Standard leaves to a user agent are taken as follows. The
//! transport layer's encoding is the caller's (`Makiri::HTML(src, encoding:)`,
//! or the encoding of a Ruby String that has a real one). The prescan reads the
//! first 1024 bytes, as the Standard encourages. Autodetection takes UTF-8 when
//! the whole input is valid UTF-8 - which the Standard's own note recommends
//! when the whole file can be examined - and asks `chardetng`, Firefox's
//! detector, otherwise; it always answers, so the implementation-defined default
//! is never reached. "Change the encoding" is [`EncodingWatch`]'s verdict;
//! the re-read it asks for - from the bytes, as the Standard asks a browser
//! to do when it cannot switch decoders on the fly - is the document parse's
//! (`html::tree::parse_document_bytes`), which this module does not drive.
//!
//! Every helper reads bytes in place: no prescan step copies a name or value
//! (the Standard lowercases them, and every comparison here is ASCII
//! case-insensitive instead), and a decoder writes into a buffer of
//! `falloc`'s.

#![forbid(unsafe_code)]

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252, X_USER_DEFINED};

use crate::falloc::try_vec_with_capacity;
use crate::html::tree::MetaProbe;

/// How far the prescan reads ("User agents are encouraged to only prescan the
/// first 1024 bytes").
const PRESCAN_LEN: usize = 1024;

/// A document parse's encoding, decided ([`sniff`]) or changed by a `<meta>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub encoding: &'static Encoding,
    /// The bytes a byte order mark takes at the start of the input.
    pub bom_len: usize,
    /// Whether the confidence is tentative: a `<meta>` may still change it.
    pub tentative: bool,
    /// Whether the sniffing found the whole input valid UTF-8 (autodetection
    /// scans it), so the parse need not scan it again.
    pub valid_utf8: bool,
}

impl Decision {
    /// What the parse reads of `input` under this decision: the bytes after
    /// any byte order mark, and whether they are known to be valid UTF-8 -
    /// found so by the sniffing, or by the caller for bytes with no BOM.
    pub fn body<'a>(&self, input: &DocumentInput<'a>) -> (&'a [u8], bool) {
        let body = input.bytes.get(self.bom_len..).unwrap_or_default();
        let known_valid = self.valid_utf8 || (input.known_valid_utf8 && self.bom_len == 0);
        (body, known_valid)
    }
}

/// A document parse's input: its bytes as the caller has them, the
/// transport layer's encoding they come under, if any, and whether they are
/// already known to be valid UTF-8 (which lets a UTF-8 parse skip its scan).
#[derive(Clone, Copy)]
pub struct DocumentInput<'a> {
    pub bytes: &'a [u8],
    pub transport: Option<&'static Encoding>,
    pub known_valid_utf8: bool,
}

/// What the String handed to a document parse says of its own encoding, as
/// the caller reads it.
#[derive(Clone, Copy)]
pub enum StringEncoding<'a> {
    /// ASCII-8BIT: bytes with no declared encoding.
    Binary,
    /// UTF-8 or US-ASCII.
    Utf8,
    /// Any other, by its name.
    Named(&'a [u8]),
}

/// The transport layer's encoding for a document parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// None: the bytes are sniffed.
    Sniff,
    /// This one, certain.
    Declared(&'static Encoding),
    /// An encoding the Standard has no label for (UTF-32, CP51932, ...),
    /// which the caller must convert to UTF-8 itself and then declare UTF-8.
    Unlabelled,
}

/// The transport layer's encoding: `given` (`encoding:`) when there is one,
/// else what the String says of itself - nothing for binary, UTF-8 for UTF-8
/// and US-ASCII, and for any other the Encoding Standard encoding its name
/// labels.
pub fn transport_for(given: Option<&'static Encoding>, string: StringEncoding<'_>) -> Transport {
    match (given, string) {
        (Some(e), _) => Transport::Declared(e),
        (None, StringEncoding::Binary) => Transport::Sniff,
        (None, StringEncoding::Utf8) => Transport::Declared(UTF_8),
        (None, StringEncoding::Named(name)) => {
            for_label(name).map_or(Transport::Unlabelled, Transport::Declared)
        }
    }
}

/// "Get an encoding" (Encoding Standard): the encoding a label names, with
/// leading and trailing ASCII whitespace ignored and ASCII case folded.
pub fn for_label(label: &[u8]) -> Option<&'static Encoding> {
    Encoding::for_label(label)
}

/// The encoding sniffing algorithm over the whole input, `transport` being
/// the transport layer's encoding. Each step's confidence is the
/// Standard's: a BOM and the transport layer are certain, the rest
/// tentative.
pub fn sniff(input: &[u8], transport: Option<&'static Encoding>) -> Decision {
    if let Some((encoding, bom_len)) = Encoding::for_bom(input) {
        return Decision {
            encoding,
            bom_len,
            tentative: false,
            valid_utf8: false,
        };
    }
    if let Some(encoding) = transport {
        return Decision {
            encoding,
            bom_len: 0,
            tentative: false,
            valid_utf8: false,
        };
    }
    if let Some(encoding) = prescan(input) {
        return Decision {
            encoding,
            bom_len: 0,
            tentative: true,
            valid_utf8: false,
        };
    }
    let (encoding, valid_utf8) = detect(input);
    Decision {
        encoding,
        bom_len: 0,
        tentative: true,
        valid_utf8,
    }
}

/// The autodetection step: UTF-8 when the input is valid UTF-8, else
/// `chardetng`'s guess. An escape byte sends even valid UTF-8 to the
/// detector: ISO-2022-JP is seven-bit, so it is valid UTF-8 too, and only its
/// escape sequences tell it apart.
/// The second value says the input was found valid UTF-8.
fn detect(input: &[u8]) -> (&'static Encoding, bool) {
    if !input.contains(&0x1B) && crate::cutf8::valid(input) {
        return (UTF_8, true);
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(input, true);
    (detector.guess(None, chardetng::Utf8Detection::Allow), false)
}

/// What a `<meta>` declaration or the prescan settles on: UTF-16BE/LE is
/// read as UTF-8, and x-user-defined as windows-1252.
fn declared(encoding: &'static Encoding) -> &'static Encoding {
    if encoding == UTF_16BE || encoding == UTF_16LE {
        UTF_8
    } else if encoding == X_USER_DEFINED {
        WINDOWS_1252
    } else {
        encoding
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// "Prescan a byte stream to determine its encoding", over its first
/// [`PRESCAN_LEN`] bytes. Running out of bytes aborts it, and then the
/// answer is "get an XML encoding" of the same bytes.
pub fn prescan(input: &[u8]) -> Option<&'static Encoding> {
    let s = input.get(..PRESCAN_LEN).unwrap_or(input);
    match prescan_meta(s) {
        Some(encoding) => Some(encoding),
        None => xml_encoding(s),
    }
}

/// The prescan's own steps: `None` where it runs out of bytes.
fn prescan_meta(s: &[u8]) -> Option<&'static Encoding> {
    if s.starts_with(&[0x3C, 0x00, 0x3F, 0x00, 0x78, 0x00]) {
        return Some(UTF_16LE);
    }
    if s.starts_with(&[0x00, 0x3C, 0x00, 0x3F, 0x00, 0x78]) {
        return Some(UTF_16BE);
    }
    let mut pos = 0;
    loop {
        let rest = s.get(pos..).filter(|r| !r.is_empty())?;
        if rest.starts_with(b"<!--") {
            /* The first `>` preceded by `--`, after the `<`; the dashes may
             * be the comment opener's own. */
            pos = (pos + 4..s.len()).find(|&j| s[j] == b'>' && s[j - 2..j] == *b"--")?;
        } else if rest.len() >= 6
            && rest[0] == b'<'
            && rest[1..5].eq_ignore_ascii_case(b"meta")
            && (is_space(rest[5]) || rest[5] == b'/')
        {
            pos += 5;
            if let Some(encoding) = meta_declaration(s, &mut pos)? {
                return Some(encoding);
            }
        } else if rest[0] == b'<' && {
            let at = if rest.get(1) == Some(&b'/') { 2 } else { 1 };
            rest.get(at).is_some_and(u8::is_ascii_alphabetic)
        } {
            pos = (pos..s.len()).find(|&j| is_space(s[j]) || s[j] == b'>')?;
            while get_attribute(s, &mut pos)?.is_some() {}
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") || rest.starts_with(b"<?") {
            pos = (pos + 1..s.len()).find(|&j| s[j] == b'>')?;
        }
        pos += 1;
    }
}

/// A `<meta>`'s attributes, from just after its name: the encoding it
/// declares, `Some(None)` when it declares none, `None` on running out of
/// bytes.
fn meta_declaration(s: &[u8], pos: &mut usize) -> Option<Option<&'static Encoding>> {
    /* Only these three names matter, so the attribute list is three flags. */
    let (mut seen_http_equiv, mut seen_content, mut seen_charset) = (false, false, false);
    let mut got_pragma = false;
    let mut need_pragma: Option<bool> = None;
    /* `None` is the Standard's null; `Some(None)` its failure. */
    let mut charset: Option<Option<&'static Encoding>> = None;
    while let Some((name, value)) = get_attribute(s, pos)? {
        if name.eq_ignore_ascii_case(b"http-equiv") {
            if !core::mem::replace(&mut seen_http_equiv, true)
                && value.eq_ignore_ascii_case(b"content-type")
            {
                got_pragma = true;
            }
        } else if name.eq_ignore_ascii_case(b"content") {
            if !core::mem::replace(&mut seen_content, true) {
                if let Some(encoding) = from_meta_content(value) {
                    if charset.is_none() {
                        charset = Some(Some(encoding));
                        need_pragma = Some(true);
                    }
                }
            }
        } else if name.eq_ignore_ascii_case(b"charset")
            && !core::mem::replace(&mut seen_charset, true)
        {
            charset = Some(for_label(value));
            need_pragma = Some(false);
        }
    }
    /* Processing. */
    match need_pragma {
        None => return Some(None),
        Some(true) if !got_pragma => return Some(None),
        _ => {}
    }
    Some(match charset {
        Some(Some(encoding)) => Some(declared(encoding)),
        _ => None,
    })
}

/// The prescan's "get an attribute": `Some(None)` when there is none (a
/// `>`), `None` on running out of bytes. The name and value are slices of
/// the input, compared ASCII case-insensitively by the caller rather than
/// lowercased into a copy.
fn get_attribute<'s>(s: &'s [u8], pos: &mut usize) -> Option<Option<(&'s [u8], &'s [u8])>> {
    while is_space(*s.get(*pos)?) || s[*pos] == b'/' {
        *pos += 1;
    }
    if s[*pos] == b'>' {
        return Some(None);
    }
    let name_start = *pos;
    loop {
        match *s.get(*pos)? {
            b'=' if *pos > name_start => {
                let name = &s[name_start..*pos];
                *pos += 1;
                return attribute_value(s, pos, name).map(Some);
            }
            b if is_space(b) => break,
            b'/' | b'>' => return Some(Some((&s[name_start..*pos], b""))),
            _ => *pos += 1,
        }
    }
    let name = &s[name_start..*pos];
    /* Spaces. */
    while is_space(*s.get(*pos)?) {
        *pos += 1;
    }
    if s[*pos] != b'=' {
        return Some(Some((name, b"")));
    }
    *pos += 1;
    attribute_value(s, pos, name).map(Some)
}

/// "Get an attribute" from its step labeled value, `name` read.
fn attribute_value<'s>(
    s: &'s [u8],
    pos: &mut usize,
    name: &'s [u8],
) -> Option<(&'s [u8], &'s [u8])> {
    while is_space(*s.get(*pos)?) {
        *pos += 1;
    }
    match s[*pos] {
        quote @ (b'"' | b'\'') => {
            let start = *pos + 1;
            let end = (start..s.len()).find(|&j| s[j] == quote)?;
            *pos = end + 1;
            Some((name, &s[start..end]))
        }
        b'>' => Some((name, b"")),
        _ => {
            let start = *pos;
            *pos += 1;
            while !(is_space(*s.get(*pos)?) || s[*pos] == b'>') {
                *pos += 1;
            }
            Some((name, &s[start..*pos]))
        }
    }
}

/// "Get an XML encoding": the `encoding` of an XML declaration at the very
/// start of `s`, the prescan's answer when it finds no `<meta>`.
fn xml_encoding(s: &[u8]) -> Option<&'static Encoding> {
    if !s.starts_with(b"<?xml") {
        return None;
    }
    let end = s.iter().position(|&b| b == b'>')?;
    let decl = &s[..end];
    let mut pos = decl.windows(8).position(|w| w == b"encoding")? + 8;
    let skip = |pos: &mut usize| {
        while s.get(*pos).is_some_and(|&b| b <= 0x20) {
            *pos += 1;
        }
    };
    skip(&mut pos);
    if s.get(pos) != Some(&b'=') {
        return None;
    }
    pos += 1;
    skip(&mut pos);
    let quote = *s.get(pos).filter(|&&q| q == b'"' || q == b'\'')?;
    pos += 1;
    let close = pos + s.get(pos..)?.iter().position(|&b| b == quote)?;
    let potential = &s[pos..close];
    if potential.iter().any(|&b| b <= 0x20) {
        return None;
    }
    let encoding = for_label(potential)?;
    Some(if encoding == UTF_16BE || encoding == UTF_16LE {
        UTF_8
    } else {
        encoding
    })
}

/// The algorithm for extracting a character encoding from a `<meta>`
/// element's `content` value.
pub fn from_meta_content(s: &[u8]) -> Option<&'static Encoding> {
    let mut pos = 0;
    loop {
        pos += s
            .get(pos..)?
            .windows(7)
            .position(|w| w.eq_ignore_ascii_case(b"charset"))?
            + 7;
        while s.get(pos).copied().is_some_and(is_space) {
            pos += 1;
        }
        if s.get(pos) != Some(&b'=') {
            continue;
        }
        pos += 1;
        while s.get(pos).copied().is_some_and(is_space) {
            pos += 1;
        }
        return match *s.get(pos)? {
            quote @ (b'"' | b'\'') => {
                let rest = &s[pos + 1..];
                let end = rest.iter().position(|&b| b == quote)?;
                for_label(&rest[..end])
            }
            _ => {
                let rest = &s[pos..];
                let end = rest
                    .iter()
                    .position(|&b| is_space(b) || b == b';')
                    .unwrap_or(rest.len());
                for_label(&rest[..end])
            }
        };
    }
}

/// The "in head" rules' watch on `<meta>` while the confidence is
/// tentative, and "change the encoding": what the document must be re-read
/// in, if anything ([`change`](Self::change)).
pub struct EncodingWatch {
    current: &'static Encoding,
    change: Option<&'static Encoding>,
}

impl EncodingWatch {
    /// A watch over a parse in `current`.
    pub fn new(current: &'static Encoding) -> Self {
        EncodingWatch {
            current,
            change: None,
        }
    }

    /// The encoding a `<meta>` changed the document to - one to re-read it
    /// in, now with the confidence certain - or `None`.
    pub fn change(&self) -> Option<&'static Encoding> {
        self.change
    }
}

impl MetaProbe for EncodingWatch {
    fn meta(
        &mut self,
        charset: Option<&[u8]>,
        http_equiv: Option<&[u8]>,
        content: Option<&[u8]>,
    ) -> bool {
        let found = charset.and_then(for_label).or_else(|| {
            http_equiv
                .filter(|v| v.eq_ignore_ascii_case(b"content-type"))
                .and(content)
                .and_then(from_meta_content)
        });
        let Some(new) = found else {
            return false;
        };
        /* "Change the encoding": certain from here on, whatever it decides. */
        if self.current == UTF_16BE || self.current == UTF_16LE {
            return true;
        }
        let new = declared(new);
        if new != self.current {
            self.change = Some(new);
        }
        true
    }
}

/// `bytes` decoded from `encoding` into UTF-8, errors replaced with U+FFFD
/// as the Encoding Standard's decoder replaces them; `None` on OOM or an
/// input too long to size the output for.
pub fn decode(bytes: &[u8], encoding: &'static Encoding) -> Option<Vec<u8>> {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let cap = decoder.max_utf8_buffer_length(bytes.len())?;
    let mut out: Vec<u8> = try_vec_with_capacity(cap)?;
    out.resize(cap, 0);
    let (result, read, written, _) = decoder.decode_to_utf8(bytes, &mut out, true);
    if result != encoding_rs::CoderResult::InputEmpty || read != bytes.len() {
        return None;
    }
    out.truncate(written);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::tree::guard::DepthLimit;
    use encoding_rs::{EUC_JP, ISO_2022_JP, SHIFT_JIS, WINDOWS_1251};

    fn tentative(input: &[u8]) -> &'static Encoding {
        let d = sniff(input, None);
        assert!(d.tentative && d.bom_len == 0);
        d.encoding
    }

    #[test]
    fn the_transport_layer_is_the_given_encoding_then_the_strings_own() {
        use StringEncoding::*;
        assert_eq!(
            transport_for(Some(EUC_JP), Utf8),
            Transport::Declared(EUC_JP)
        );
        assert_eq!(transport_for(None, Binary), Transport::Sniff);
        assert_eq!(transport_for(None, Utf8), Transport::Declared(UTF_8));
        assert_eq!(
            transport_for(None, Named(b"Windows-31J")),
            Transport::Declared(SHIFT_JIS)
        );
        assert_eq!(
            transport_for(None, Named(b"ISO-8859-1")),
            Transport::Declared(WINDOWS_1252)
        );
        assert_eq!(
            transport_for(None, Named(b"UTF-32LE")),
            Transport::Unlabelled
        );
    }

    #[test]
    fn a_bom_wins_over_everything_and_is_certain() {
        let d = sniff(b"\xEF\xBB\xBF<meta charset=euc-jp>", Some(SHIFT_JIS));
        assert_eq!((d.encoding, d.bom_len, d.tentative), (UTF_8, 3, false));
        assert_eq!(sniff(b"\xFF\xFE<\0", None).encoding, UTF_16LE);
        assert_eq!(sniff(b"\xFE\xFF\0<", None).encoding, UTF_16BE);
    }

    #[test]
    fn the_transport_layer_is_certain_and_beats_a_meta() {
        let d = sniff(b"<meta charset=euc-jp>", Some(SHIFT_JIS));
        assert_eq!((d.encoding, d.tentative), (SHIFT_JIS, false));
    }

    #[test]
    fn the_prescan_reads_meta_charset_and_pragma() {
        assert_eq!(tentative(b"<meta charset=euc-jp>"), EUC_JP);
        assert_eq!(tentative(b"<META CHARSET='Shift_JIS'>"), SHIFT_JIS);
        assert_eq!(
            tentative(b"<meta\tcharset = \" koi8-r \">"),
            encoding_rs::KOI8_R
        );
        assert_eq!(
            tentative(b"<meta http-equiv=Content-Type content='text/html; charset=euc-jp'>"),
            EUC_JP
        );
        // `content` without the pragma, or the pragma after a charset
        // attribute decided it: the Standard's need-pragma rules.
        assert_eq!(tentative(b"<meta content='charset=euc-jp'><p>"), UTF_8);
        assert_eq!(
            tentative(b"<meta content='charset=euc-jp' charset=windows-1251>"),
            WINDOWS_1251
        );
        // A repeated attribute name is ignored after the first.
        assert_eq!(tentative(b"<meta charset=x-bogus charset=euc-jp>"), UTF_8);
        // UTF-16 and x-user-defined are declared as UTF-8 and windows-1252.
        assert_eq!(tentative(b"<meta charset=utf-16le>"), UTF_8);
        assert_eq!(tentative(b"<meta charset=x-user-defined>"), WINDOWS_1252);
    }

    #[test]
    fn the_prescan_skips_comments_tags_and_their_attributes() {
        assert_eq!(tentative(b"<!-- <meta charset=euc-jp> --><p>"), UTF_8);
        assert_eq!(tentative(b"<!--><meta charset=euc-jp>"), EUC_JP);
        assert_eq!(tentative(b"<p title='<meta charset=euc-jp>'>"), UTF_8);
        assert_eq!(
            tentative(b"<?pi <meta charset=x>?><meta charset=euc-jp>"),
            EUC_JP
        );
        assert_eq!(
            tentative(b"</p x='<meta charset=a>'><meta charset=euc-jp>"),
            EUC_JP
        );
        assert_eq!(tentative(b"<metal charset=euc-jp>"), UTF_8);
    }

    #[test]
    fn the_prescan_stops_at_1024_bytes() {
        let mut late = vec![b' '; 1020];
        late.extend_from_slice(b"<meta charset=euc-jp>");
        assert_eq!(tentative(&late), UTF_8);
        let mut early = vec![b' '; 1000];
        early.extend_from_slice(b"<meta charset=euc-jp>");
        assert_eq!(tentative(&early), EUC_JP);
    }

    #[test]
    fn an_xml_declaration_is_the_fallback() {
        assert_eq!(
            tentative(b"<?xml version='1.0' encoding='euc-jp'?><p>"),
            EUC_JP
        );
        assert_eq!(tentative(b"<?xml encoding=\"utf-16\"?>"), UTF_8);
        assert_eq!(tentative(b" <?xml encoding='euc-jp'?>"), UTF_8);
        assert_eq!(
            tentative(b"<?xml encoding='euc-jp'?><meta charset=koi8-r>"),
            encoding_rs::KOI8_R
        );
        assert_eq!(tentative(b"<\0?\0x\0m\0l\0"), UTF_16LE);
    }

    #[test]
    fn extracting_from_content_follows_the_standard() {
        assert_eq!(
            from_meta_content(b"text/html; charset=euc-jp"),
            Some(EUC_JP)
        );
        assert_eq!(
            from_meta_content(b"charsetX; charset = \"euc-jp\""),
            Some(EUC_JP)
        );
        assert_eq!(from_meta_content(b"charset='euc-jp"), None);
        assert_eq!(from_meta_content(b"charset="), None);
        assert_eq!(from_meta_content(b"CHARSET=EUC-JP;x"), Some(EUC_JP));
        assert_eq!(from_meta_content(b"text/html"), None);
    }

    #[test]
    fn detection_takes_utf8_then_asks_the_detector() {
        assert_eq!(tentative("<p>日本語</p>".as_bytes()), UTF_8);
        assert_eq!(tentative(b"<p>plain</p>"), UTF_8);
        let sjis = SHIFT_JIS
            .encode("<p>日本語のテキスト、ひらがなとカタカナ。</p>")
            .0;
        assert_eq!(tentative(&sjis), SHIFT_JIS);
        let jis = ISO_2022_JP
            .encode("<p>日本語のテキスト、ひらがなとカタカナ。</p>")
            .0;
        assert_eq!(tentative(&jis), ISO_2022_JP);
    }

    /// The UTF-8 path skips the decoder for the parser's own sanitiser, so
    /// the two must replace invalid input identically.
    #[test]
    fn the_sanitiser_is_the_utf8_decoder() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let pool: &[u8] = &[
            0x41, 0x80, 0xBF, 0xC0, 0xC2, 0xDF, 0xE0, 0xE1, 0xED, 0xEF, 0xF0, 0xF4, 0xF5, 0xFF,
            0x9F, 0xA0,
        ];
        for _ in 0..20_000 {
            let len = (next() % 12) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| pool[(next() % pool.len() as u64) as usize])
                .collect();
            let ours = crate::utf8_input::sanitize(&bytes, false).expect("memory");
            let theirs = decode(&bytes, UTF_8).expect("memory");
            assert_eq!(ours.as_slice(), &theirs[..], "{bytes:02X?}");
        }
    }

    #[test]
    fn a_meta_while_tentative_changes_the_encoding() {
        let mut src = b"<!--".to_vec();
        src.extend(vec![b'x'; 1100]);
        src.extend_from_slice(b"--><meta charset=euc-jp><p>");
        src.extend_from_slice(&EUC_JP.encode("日本語").0);
        let parse = |transport| {
            let input = DocumentInput {
                bytes: &src,
                transport,
                known_valid_utf8: false,
            };
            crate::html::tree::parse_document_bytes(input, DepthLimit::at_most(400))
                .expect("parse")
                .encoding()
        };
        assert_eq!(parse(None), "EUC-JP");
        // Certain from the transport layer: the meta changes nothing.
        assert_eq!(parse(Some(UTF_8)), "UTF-8");
    }
}
