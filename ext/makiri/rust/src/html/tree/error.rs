//! The tree builder's parse errors (`html/tree/error.h`).

/// `lxb_html_tree_error_id_t`, in Lexbor's order, so `id as u32` is
/// Lexbor's number.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TreeErrorId {
    /// unexpected-token
    Unto = 0,
    /// unexpected-closed-token
    Unclto,
    /// null-character
    Nuch,
    /// unexpected-character-token
    Unchto,
    /// unexpected-token-in-initial-mode
    Untoininmo,
    /// bad-doctype-token-in-initial-mode
    Badotoininmo,
    /// doctype-token-in-before-html-mode
    Dotoinbehtmo,
    /// unexpected-closed-token-in-before-html-mode
    Uncltoinbehtmo,
    /// doctype-token-in-before-head-mode
    Dotoinbehemo,
    /// unexpected-closed_token-in-before-head-mode
    Uncltoinbehemo,
    /// doctype-token-in-head-mode
    Dotoinhemo,
    /// non-void-html-element-start-tag-with-trailing-solidus
    Novohtelsttawitrso,
    /// head-token-in-head-mode
    Hetoinhemo,
    /// unexpected-closed-token-in-head-mode
    Uncltoinhemo,
    /// template-closed-token-without-opening-in-head-mode
    Tecltowiopinhemo,
    /// template-element-is-not-current-in-head-mode
    Teelisnocuinhemo,
    /// doctype-token-in-head-noscript-mode
    Dotoinhenomo,
    /// doctype-token-after-head-mode
    Dotoafhemo,
    /// head-token-after-head-mode
    Hetoafhemo,
    /// doctype-token-in-body-mode
    Dotoinbomo,
    /// bad-ending-open-elements-is-wrong
    Baenopeliswr,
    /// open-elements-is-wrong
    Opeliswr,
    /// unexpected-element-in-open-elements-stack
    Unelinopelst,
    /// missing-element-in-open-elements-stack
    Mielinopelst,
    /// no-body-element-in-scope
    Noboelinsc,
    /// missing-element-in-scope
    Mielinsc,
    /// unexpected-element-in-scope
    Unelinsc,
    /// unexpected-element-in-active-formatting-stack
    Unelinacfost,
    /// unexpected-end-of-file
    Unenoffi,
    /// characters-in-table-text
    Chintate,
    /// doctype-token-in-table-mode
    Dotointamo,
    /// doctype-token-in-select-mode
    Dotoinsemo,
    /// doctype-token-after-body-mode
    Dotoafbomo,
    /// doctype-token-in-frameset-mode
    Dotoinfrmo,
    /// doctype-token-after-frameset-mode
    Dotoaffrmo,
    /// doctype-token-foreign-content-mode
    Dotofocomo,
    /// select-in-scope
    Seinsc,
    /// fragment-parsing-select-in-context-parse-input
    Frpaseincopain,
    /// fragment-parsing-select-in-context-parse-select
    Frpaseincopase,
    /// hr-parsing-select-option-optgroup-in-scope
    Hrpaseopopinsc,
    /// option-parsing-option-in-scope
    Oppaopinsc,
    /// optgroup-parsing-option-optgroup-in-scope
    Oppaopopinsc,
}

/// One recorded parse error (`lxb_html_tree_error_t`): the id and the
/// token's `begin` and `end` at the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeError {
    pub id: TreeErrorId,
    pub begin: Option<usize>,
    pub end: Option<usize>,
}
