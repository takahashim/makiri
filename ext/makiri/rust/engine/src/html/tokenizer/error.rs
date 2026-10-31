//! `lxb_html_tokenizer_error_id_t`, in Lexbor's order, each with the HTML
//! Standard's name for the parse error.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorId {
    /// abrupt-closing-of-empty-comment
    Abclofemco,
    /// abrupt-doctype-public-identifier
    Abdopuid,
    /// abrupt-doctype-system-identifier
    Abdosyid,
    /// absence-of-digits-in-numeric-character-reference
    Abofdiinnuchre,
    /// cdata-in-html-content
    Cdinhtco,
    /// character-reference-outside-unicode-range
    Chreouunra,
    /// control-character-in-input-stream
    Cochininst,
    /// control-character-reference
    Cochre,
    /// end-tag-with-attributes
    Entawiat,
    /// duplicate-attribute
    Duat,
    /// end-tag-with-trailing-solidus
    Entawitrso,
    /// eof-before-tag-name
    Eobetana,
    /// eof-in-cdata
    Eoincd,
    /// eof-in-comment
    Eoinco,
    /// eof-in-doctype
    Eoindo,
    /// eof-in-script-html-comment-like-text
    Eoinschtcolite,
    /// eof-in-tag
    Eointa,
    /// incorrectly-closed-comment
    Inclco,
    /// incorrectly-opened-comment
    Inopco,
    /// invalid-character-sequence-after-doctype-name
    Inchseafdona,
    /// invalid-first-character-of-tag-name
    Infichoftana,
    /// missing-attribute-value
    Miatva,
    /// missing-doctype-name
    Midona,
    /// missing-doctype-public-identifier
    Midopuid,
    /// missing-doctype-system-identifier
    Midosyid,
    /// missing-end-tag-name
    Mientana,
    /// missing-quote-before-doctype-public-identifier
    Miqubedopuid,
    /// missing-quote-before-doctype-system-identifier
    Miqubedosyid,
    /// missing-semicolon-after-character-reference
    Miseafchre,
    /// missing-whitespace-after-doctype-public-keyword
    Miwhafdopuke,
    /// missing-whitespace-after-doctype-system-keyword
    Miwhafdosyke,
    /// missing-whitespace-before-doctype-name
    Miwhbedona,
    /// missing-whitespace-between-attributes
    Miwhbeat,
    /// missing-whitespace-between-doctype-public-and-system-identifiers
    Miwhbedopuansyid,
    /// nested-comment
    Neco,
    /// noncharacter-character-reference
    Nochre,
    /// noncharacter-in-input-stream
    Noininst,
    /// non-void-html-element-start-tag-with-trailing-solidus
    Novohtelsttawitrso,
    /// null-character-reference
    Nuchre,
    /// surrogate-character-reference
    Suchre,
    /// surrogate-in-input-stream
    Suininst,
    /// unexpected-character-after-doctype-system-identifier
    Unchafdosyid,
    /// unexpected-character-in-attribute-name
    Unchinatna,
    /// unexpected-character-in-unquoted-attribute-value
    Unchinunatva,
    /// unexpected-equals-sign-before-attribute-name
    Uneqsibeatna,
    /// unexpected-null-character
    Unnuch,
    /// unexpected-question-mark-instead-of-tag-name
    Unqumainoftana,
    /// unexpected-solidus-in-tag
    Unsointa,
    /// unknown-named-character-reference
    Unnachre,
    /// eof-in-processing-instruction
    Eoinprin,
    /// invalid-first-character-of-processing-instruction-target
    Infichofprinta,
    /// disallowed-processing-instruction-target
    Diprinta,
    /// invalid-processing-instruction-target
    Inprinta,
}
