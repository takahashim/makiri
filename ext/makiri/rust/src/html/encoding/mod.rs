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
//! detector, otherwise, over the first 64 KiB from the first byte that is not
//! ASCII ([`DETECT_MAX`]); it always answers, so the implementation-defined
//! default is never reached. "Change the encoding" is [`EncodingWatch`]'s verdict;
//! the re-read it asks for - from the bytes, as the Standard asks a browser
//! to do when it cannot switch decoders on the fly - is the document parse's
//! (`html::tree::parse_document_bytes`), which this module does not drive.
//!
//! The prescan is a parser of its own, the Standard's steps over the first
//! bytes ([`prescan`]'s module); this one holds the policy around it and the
//! decoders, which write into a buffer of `falloc`'s.

#![forbid(unsafe_code)]

use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED};

/// The Encoding Standard's encodings, as the rest of the crate names them:
/// through this module, the one that reads `encoding_rs`.
pub use encoding_rs::{Encoding, UTF_8};

use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::html::tree::{MetaProbe, MetaVerdict};

mod prescan;
pub use prescan::{from_meta_content, prescan};

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
    /// A decision the confidence in is certain: a byte order mark of
    /// `bom_len` bytes, the transport layer, or a `<meta>` that changed the
    /// encoding (the re-read, which skips the sniffing).
    pub fn certain(encoding: &'static Encoding, bom_len: usize) -> Self {
        Decision {
            encoding,
            bom_len,
            tentative: false,
            valid_utf8: false,
        }
    }

    /// A tentative decision - the prescan's or the detector's - which a
    /// `<meta>` may still change; `valid_utf8` when the sniffing found the
    /// whole input valid UTF-8.
    fn tentative(encoding: &'static Encoding, valid_utf8: bool) -> Self {
        Decision {
            encoding,
            bom_len: 0,
            tentative: true,
            valid_utf8,
        }
    }

    /// What the parse reads of `input` under this decision: the bytes after
    /// any byte order mark, and whether they are known to be valid UTF-8 -
    /// found so by the sniffing, or by the caller. Only bytes read AS UTF-8
    /// can be known so: any other encoding is decoded, and its decoder's
    /// output is valid by construction. A UTF-8 BOM is a whole character, so
    /// what follows one in valid input is valid too.
    pub fn body<'a>(&self, input: &DocumentInput<'a>) -> (&'a [u8], bool) {
        let body = input.bytes.get(self.bom_len..).unwrap_or_default();
        let known_valid = self.encoding == UTF_8 && (self.valid_utf8 || input.known_valid_utf8);
        (body, known_valid)
    }
}

/// A document parse's input: its bytes as the caller has them, the
/// transport layer's encoding they come under, if any, and whether they are
/// already known to be valid UTF-8 - which lets a parse that reads them as
/// UTF-8 skip its scan, and is consulted for no other ([`Decision::body`]).
#[derive(Clone, Copy)]
pub struct DocumentInput<'a> {
    pub bytes: &'a [u8],
    pub transport: Option<&'static Encoding>,
    pub known_valid_utf8: bool,
}

/// A Ruby encoding, as a document parse reads it: the String's own, or the
/// one `encoding:` names when Ruby knows that name. Both are read by the one
/// rule, [`transport_for`], so a String tagged US-ASCII and `encoding:
/// "US-ASCII"` are the same input.
#[derive(Clone, Copy)]
pub enum StringEncoding<'a> {
    /// ASCII-8BIT or US-ASCII: bytes with no claim about their encoding
    /// (US-ASCII being what `File.read` gives any bytes under `LANG=C`).
    Binary,
    /// UTF-8.
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

/// The transport layer's encoding a Ruby encoding stands for: nothing for
/// binary (the bytes are sniffed), UTF-8 for UTF-8, and for any other the
/// Encoding Standard encoding its name labels.
pub fn transport_for(string: StringEncoding<'_>) -> Transport {
    match string {
        StringEncoding::Binary => Transport::Sniff,
        StringEncoding::Utf8 => Transport::Declared(UTF_8),
        StringEncoding::Named(name) => {
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
        return Decision::certain(encoding, bom_len);
    }
    if let Some(encoding) = transport {
        return Decision::certain(encoding, 0);
    }
    if let Some(encoding) = prescan(input) {
        return Decision::tentative(encoding, false);
    }
    let (encoding, valid_utf8) = detect(input);
    Decision::tentative(encoding, valid_utf8)
}

/// How much of the input [`detect`] hands `chardetng`, from the first byte
/// that is not ASCII (or is an escape).
///
/// The detector reads ~20 MB/s, a tenth of the parser's speed, so a whole
/// undeclared 8 MB page spent half a second guessing - and any input could
/// be made to. Its guess settles long before: over sample pages in fifteen
/// legacy encodings (`notes/doc_order_bench/detect_caps.rb`) every cap from
/// 1 KiB guessed as the whole input did; this one leaves a wide margin.
const DETECT_MAX: usize = 64 * 1024;

/// The autodetection step: UTF-8 when the input is valid UTF-8, else
/// `chardetng`'s guess, with whether the input was found valid UTF-8. An
/// escape byte sends even valid UTF-8 to the detector: ISO-2022-JP is
/// seven-bit, so it is valid UTF-8 too, and only its escape sequences tell
/// it apart.
///
/// The UTF-8 test reads the whole input (it is fast); the detector reads
/// [`DETECT_MAX`] bytes from the first byte that is not ASCII or is an
/// escape. The ASCII before it - a page's inline scripts and styles - tells
/// nothing apart, and starting there is starting on a character boundary in
/// every encoding the detector guesses. A cut short of the end is not
/// reported as the end (`last`), so an ISO-2022-JP sequence left open by the
/// cut does not count against it.
fn detect(input: &[u8]) -> (&'static Encoding, bool) {
    if !input.contains(&0x1B) && crate::cutf8::valid(input) {
        return (UTF_8, true);
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    let from = input
        .iter()
        .position(|&b| b >= 0x80 || b == 0x1B)
        .unwrap_or(input.len());
    let to = input.len().min(from.saturating_add(DETECT_MAX));
    detector.feed(input.get(from..to).unwrap_or_default(), to == input.len());
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
    ) -> MetaVerdict {
        let found = charset.and_then(for_label).or_else(|| {
            http_equiv
                .filter(|v| v.eq_ignore_ascii_case(b"content-type"))
                .and(content)
                .and_then(from_meta_content)
        });
        let Some(new) = found else {
            return MetaVerdict::Tentative;
        };
        /* "Change the encoding": certain from here on, whatever it decides. */
        if self.current == UTF_16BE || self.current == UTF_16LE {
            return MetaVerdict::Certain;
        }
        let new = declared(new);
        if new == self.current {
            return MetaVerdict::Certain;
        }
        self.change = Some(new);
        MetaVerdict::Reread
    }
}

/// How much room [`decode`] gives the decoder at a time.
const DECODE_CHUNK: usize = 16 * 1024;

/// `bytes` decoded from `encoding` into UTF-8, errors replaced with U+FFFD
/// as the Encoding Standard's decoder replaces them; `None` on OOM.
///
/// Decoded a chunk at a time into the output's own tail, which grows as
/// the text does: sizing it for the worst case up front zeroed and held
/// three times a single-byte document's length for the whole parse.
pub fn decode(bytes: &[u8], encoding: &'static Encoding) -> Option<Vec<u8>> {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let mut out: Vec<u8> = try_vec_with_capacity(bytes.len().checked_add(DECODE_CHUNK)?)?;
    let mut rest = bytes;
    loop {
        let len = out.len();
        out.falloc_reserve(DECODE_CHUNK).ok()?;
        out.resize(len + DECODE_CHUNK, 0);
        let (result, read, written, _) = decoder.decode_to_utf8(rest, out.get_mut(len..)?, true);
        out.truncate(len + written);
        rest = rest.get(read..)?;
        if result == encoding_rs::CoderResult::InputEmpty {
            return Some(out);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::*;
    use crate::html::tree::guard::DepthLimit;
    use encoding_rs::{EUC_JP, ISO_2022_JP, SHIFT_JIS, WINDOWS_1251};

    fn tentative(input: &[u8]) -> &'static Encoding {
        let d = sniff(input, None);
        assert!(d.tentative && d.bom_len == 0);
        d.encoding
    }

    #[test]
    fn the_transport_layer_is_what_the_ruby_encoding_stands_for() {
        use StringEncoding::*;
        assert_eq!(transport_for(Binary), Transport::Sniff);
        assert_eq!(transport_for(Utf8), Transport::Declared(UTF_8));
        assert_eq!(
            transport_for(Named(b"Windows-31J")),
            Transport::Declared(SHIFT_JIS)
        );
        assert_eq!(
            transport_for(Named(b"ISO-8859-1")),
            Transport::Declared(WINDOWS_1252)
        );
        assert_eq!(transport_for(Named(b"UTF-32LE")), Transport::Unlabelled);
    }

    /// `text` repeated past `n` bytes once encoded in `e`.
    fn encoded_past(text: &str, e: &'static Encoding, n: usize) -> Vec<u8> {
        let one = e.encode(text).0.into_owned();
        let mut out = Vec::new();
        while out.len() <= n {
            out.extend_from_slice(&one);
        }
        out
    }

    const JA: &str = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。\n";

    #[test]
    fn the_detector_starts_past_an_ascii_head() {
        let mut input = b"<script>".to_vec();
        input.resize(DETECT_MAX * 2, b' ');
        input.extend(encoded_past(JA, SHIFT_JIS, 4096));
        assert_eq!(detect(&input), (SHIFT_JIS, false));
    }

    #[test]
    fn the_detector_reads_only_its_window() {
        let mut input = encoded_past(JA, EUC_JP, DETECT_MAX);
        input.extend(encoded_past(JA, SHIFT_JIS, DETECT_MAX * 4));
        /* Fed all of it, the detector guesses otherwise (GBK, as it
         * happens): what it answers here is the window's alone. */
        let mut whole = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
        whole.feed(&input, true);
        assert_ne!(whole.guess(None, chardetng::Utf8Detection::Allow), EUC_JP);
        assert_eq!(detect(&input).0, EUC_JP);
    }

    #[test]
    fn a_window_cut_inside_iso_2022_jp_is_not_its_end() {
        /* The window ends at every offset into a line - mid-escape, mid-
         * character, in JIS mode - by shifting the lines after the first. */
        let line = ISO_2022_JP.encode(JA).0.into_owned();
        for shift in 0..line.len() {
            let mut input = line.clone();
            input.resize(line.len() + shift, b'x');
            input.extend(encoded_past(JA, ISO_2022_JP, DETECT_MAX * 2));
            assert_eq!(detect(&input).0, ISO_2022_JP, "shifted by {shift}");
        }
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
