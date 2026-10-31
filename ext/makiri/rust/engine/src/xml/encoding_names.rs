//! XML 1.0 §4.3.3: an encoding declaration names its encoding by an IANA
//! charset name or alias, matched without regard to ASCII case. Ruby's
//! `Encoding.find` knows most of them; [`ruby_name`] supplies the registered
//! aliases it does not (`latin1`, `l1`, `csISOLatin1`, `IBM367`, ...), by the
//! name Ruby knows the same charset under. A name in neither is not one XML
//! registers (`utf8`), and the declaration is a fatal error.
//!
//! The table is the IANA Character Sets registry
//! (https://www.iana.org/assignments/character-sets/) less every name
//! `Encoding.find` already resolves, keyed by the lower-cased alias and
//! sorted for a binary search.

#![forbid(unsafe_code)]

/// The name Ruby knows the charset `alias` is registered for, when Ruby
/// does not know `alias` itself.
pub fn ruby_name(alias: &[u8]) -> Option<&'static [u8]> {
    let mut key = [0u8; 64];
    let key = key.get_mut(..alias.len())?;
    for (k, b) in key.iter_mut().zip(alias) {
        *k = b.to_ascii_lowercase();
    }
    IANA_ALIASES
        .binary_search_by(|(a, _)| (*a).cmp(&*key))
        .ok()
        .and_then(|i| IANA_ALIASES.get(i))
        .map(|(_, name)| *name)
}

/// Whether `name` is one of Ruby's names for the process's own encodings,
/// which `Encoding.find` resolves but which name no charset.
pub fn is_ruby_special(name: &[u8]) -> bool {
    [&b"locale"[..], b"external", b"internal", b"filesystem"]
        .iter()
        .any(|s| name.eq_ignore_ascii_case(s))
}

/// `(lower-cased IANA alias, Ruby's name for its charset)`, sorted.
const IANA_ALIASES: &[(&[u8], &[u8])] = &[
    (b"437", b"IBM437"),
    (b"850", b"CP850"),
    (b"852", b"IBM852"),
    (b"855", b"IBM855"),
    (b"857", b"IBM857"),
    (b"860", b"IBM860"),
    (b"861", b"IBM861"),
    (b"862", b"IBM862"),
    (b"863", b"IBM863"),
    (b"865", b"IBM865"),
    (b"866", b"IBM866"),
    (b"869", b"IBM869"),
    (b"ansi_x3.4-1986", b"US-ASCII"),
    (b"arabic", b"ISO-8859-6"),
    (b"asmo-708", b"ISO-8859-6"),
    (b"cp-gr", b"IBM869"),
    (b"cp-is", b"IBM861"),
    (b"cp037", b"IBM037"),
    (b"cp367", b"US-ASCII"),
    (b"cp819", b"ISO-8859-1"),
    (b"csascii", b"US-ASCII"),
    (b"csbig5", b"Big5"),
    (b"csbig5hkscs", b"Big5-HKSCS"),
    (b"cscesu-8", b"CESU-8"),
    (b"cscesu8", b"CESU-8"),
    (b"cscp50220", b"CP50220"),
    (b"cscp51932", b"CP51932"),
    (b"cseuckr", b"EUC-KR"),
    (b"cseucpkdfmtjapanese", b"EUC-JP"),
    (b"csgb18030", b"GB18030"),
    (b"csgb2312", b"GB2312"),
    (b"csgbk", b"GBK"),
    (b"csibm037", b"IBM037"),
    (b"csibm855", b"IBM855"),
    (b"csibm857", b"IBM857"),
    (b"csibm860", b"IBM860"),
    (b"csibm861", b"IBM861"),
    (b"csibm863", b"IBM863"),
    (b"csibm864", b"IBM864"),
    (b"csibm865", b"IBM865"),
    (b"csibm866", b"IBM866"),
    (b"csibm869", b"IBM869"),
    (b"csiso2022jp", b"ISO-2022-JP"),
    (b"csiso2022jp2", b"ISO-2022-JP-2"),
    (b"csiso885913", b"ISO-8859-13"),
    (b"csiso885914", b"ISO-8859-14"),
    (b"csiso885915", b"ISO-8859-15"),
    (b"csiso885916", b"ISO-8859-16"),
    (b"csisolatin1", b"ISO-8859-1"),
    (b"csisolatin2", b"ISO-8859-2"),
    (b"csisolatin3", b"ISO-8859-3"),
    (b"csisolatin4", b"ISO-8859-4"),
    (b"csisolatin5", b"ISO-8859-9"),
    (b"csisolatin6", b"ISO-8859-10"),
    (b"csisolatinarabic", b"ISO-8859-6"),
    (b"csisolatincyrillic", b"ISO-8859-5"),
    (b"csisolatingreek", b"ISO-8859-7"),
    (b"csisolatinhebrew", b"ISO-8859-8"),
    (b"cskoi8r", b"KOI8-R"),
    (b"cskoi8u", b"KOI8-U"),
    (b"cspc775baltic", b"IBM775"),
    (b"cspc850multilingual", b"CP850"),
    (b"cspc862latinhebrew", b"IBM862"),
    (b"cspc8codepage437", b"IBM437"),
    (b"cspcp852", b"IBM852"),
    (b"csshiftjis", b"Shift_JIS"),
    (b"cstis620", b"TIS-620"),
    (b"csutf16", b"UTF-16"),
    (b"csutf16be", b"UTF-16BE"),
    (b"csutf16le", b"UTF-16LE"),
    (b"csutf32", b"UTF-32"),
    (b"csutf32be", b"UTF-32BE"),
    (b"csutf32le", b"UTF-32LE"),
    (b"csutf7", b"UTF-7"),
    (b"csutf8", b"UTF-8"),
    (b"cswindows1250", b"Windows-1250"),
    (b"cswindows1251", b"Windows-1251"),
    (b"cswindows1252", b"Windows-1252"),
    (b"cswindows1253", b"Windows-1253"),
    (b"cswindows1254", b"Windows-1254"),
    (b"cswindows1255", b"Windows-1255"),
    (b"cswindows1256", b"Windows-1256"),
    (b"cswindows1257", b"Windows-1257"),
    (b"cswindows1258", b"Windows-1258"),
    (b"cswindows874", b"Windows-874"),
    (b"cyrillic", b"ISO-8859-5"),
    (b"ebcdic-cp-ca", b"IBM037"),
    (b"ebcdic-cp-nl", b"IBM037"),
    (b"ebcdic-cp-wt", b"IBM037"),
    (b"ecma-114", b"ISO-8859-6"),
    (b"ecma-118", b"ISO-8859-7"),
    (b"elot_928", b"ISO-8859-7"),
    (b"extended_unix_code_packed_format_for_japanese", b"EUC-JP"),
    (b"greek", b"ISO-8859-7"),
    (b"greek8", b"ISO-8859-7"),
    (b"hebrew", b"ISO-8859-8"),
    (b"ibm367", b"US-ASCII"),
    (b"ibm819", b"ISO-8859-1"),
    (b"iso-celtic", b"ISO-8859-14"),
    (b"iso-ir-100", b"ISO-8859-1"),
    (b"iso-ir-101", b"ISO-8859-2"),
    (b"iso-ir-109", b"ISO-8859-3"),
    (b"iso-ir-110", b"ISO-8859-4"),
    (b"iso-ir-126", b"ISO-8859-7"),
    (b"iso-ir-127", b"ISO-8859-6"),
    (b"iso-ir-138", b"ISO-8859-8"),
    (b"iso-ir-144", b"ISO-8859-5"),
    (b"iso-ir-148", b"ISO-8859-9"),
    (b"iso-ir-157", b"ISO-8859-10"),
    (b"iso-ir-199", b"ISO-8859-14"),
    (b"iso-ir-226", b"ISO-8859-16"),
    (b"iso-ir-6", b"US-ASCII"),
    (b"iso646-us", b"US-ASCII"),
    (b"iso_646.irv:1991", b"US-ASCII"),
    (b"iso_8859-1", b"ISO-8859-1"),
    (b"iso_8859-10:1992", b"ISO-8859-10"),
    (b"iso_8859-14", b"ISO-8859-14"),
    (b"iso_8859-14:1998", b"ISO-8859-14"),
    (b"iso_8859-15", b"ISO-8859-15"),
    (b"iso_8859-16", b"ISO-8859-16"),
    (b"iso_8859-16:2001", b"ISO-8859-16"),
    (b"iso_8859-1:1987", b"ISO-8859-1"),
    (b"iso_8859-2", b"ISO-8859-2"),
    (b"iso_8859-2:1987", b"ISO-8859-2"),
    (b"iso_8859-3", b"ISO-8859-3"),
    (b"iso_8859-3:1988", b"ISO-8859-3"),
    (b"iso_8859-4", b"ISO-8859-4"),
    (b"iso_8859-4:1988", b"ISO-8859-4"),
    (b"iso_8859-5", b"ISO-8859-5"),
    (b"iso_8859-5:1988", b"ISO-8859-5"),
    (b"iso_8859-6", b"ISO-8859-6"),
    (b"iso_8859-6:1987", b"ISO-8859-6"),
    (b"iso_8859-7", b"ISO-8859-7"),
    (b"iso_8859-7:1987", b"ISO-8859-7"),
    (b"iso_8859-8", b"ISO-8859-8"),
    (b"iso_8859-8:1988", b"ISO-8859-8"),
    (b"iso_8859-9", b"ISO-8859-9"),
    (b"iso_8859-9:1989", b"ISO-8859-9"),
    (b"l1", b"ISO-8859-1"),
    (b"l10", b"ISO-8859-16"),
    (b"l2", b"ISO-8859-2"),
    (b"l3", b"ISO-8859-3"),
    (b"l4", b"ISO-8859-4"),
    (b"l5", b"ISO-8859-9"),
    (b"l6", b"ISO-8859-10"),
    (b"l8", b"ISO-8859-14"),
    (b"latin-9", b"ISO-8859-15"),
    (b"latin1", b"ISO-8859-1"),
    (b"latin10", b"ISO-8859-16"),
    (b"latin2", b"ISO-8859-2"),
    (b"latin3", b"ISO-8859-3"),
    (b"latin4", b"ISO-8859-4"),
    (b"latin5", b"ISO-8859-9"),
    (b"latin6", b"ISO-8859-10"),
    (b"latin8", b"ISO-8859-14"),
    (b"ms936", b"GBK"),
    (b"ms_kanji", b"Shift_JIS"),
    (b"us", b"US-ASCII"),
    (b"windows-936", b"GBK"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_lower_cased() {
        assert!(IANA_ALIASES.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(IANA_ALIASES
            .iter()
            .all(|(a, _)| !a.iter().any(u8::is_ascii_uppercase)));
    }

    #[test]
    fn an_alias_is_found_whatever_its_case() {
        assert_eq!(ruby_name(b"latin1"), Some(&b"ISO-8859-1"[..]));
        assert_eq!(ruby_name(b"csISOLatin1"), Some(&b"ISO-8859-1"[..]));
        assert_eq!(ruby_name(b"IBM367"), Some(&b"US-ASCII"[..]));
        assert_eq!(ruby_name(b"utf8"), None);
        assert_eq!(ruby_name(&[b'x'; 100]), None);
    }
}
