//! The pseudo-class and pseudo-element tables: Lexbor's
//! `css/selectors/pseudo_const.h`, `pseudo_res.h` and `pseudo.c`.
//!
//! The ids are Lexbor's (the enums' order), so a parsed selector carries the
//! numbers Lexbor's does. A name is looked up ASCII-case-insensitively, as
//! `lexbor_shs_entry_get_lower_static` does.
//!
//! `lexbor-contains` is left out of the function table: Makiri never let it
//! reach Lexbor's parser (`lexbor::contains_guard` renamed it), so to the port
//! it is an unknown function, which is what the guarded parser made it.

use super::ast::comb;
use super::parser::{EndCb, St};

/// `lxb_css_selector_pseudo_class_id_t`.
pub mod class {
    pub const UNDEF: u32 = 0;
    pub const ACTIVE: u32 = 1;
    pub const ANY_LINK: u32 = 2;
    pub const BLANK: u32 = 3;
    pub const CHECKED: u32 = 4;
    pub const CURRENT: u32 = 5;
    pub const DEFAULT: u32 = 6;
    pub const DISABLED: u32 = 7;
    pub const EMPTY: u32 = 8;
    pub const ENABLED: u32 = 9;
    pub const FIRST_CHILD: u32 = 10;
    pub const FIRST_OF_TYPE: u32 = 11;
    pub const FOCUS: u32 = 12;
    pub const FOCUS_VISIBLE: u32 = 13;
    pub const FOCUS_WITHIN: u32 = 14;
    pub const FULLSCREEN: u32 = 15;
    pub const FUTURE: u32 = 16;
    pub const HOVER: u32 = 17;
    pub const IN_RANGE: u32 = 18;
    pub const INDETERMINATE: u32 = 19;
    pub const INVALID: u32 = 20;
    pub const LAST_CHILD: u32 = 21;
    pub const LAST_OF_TYPE: u32 = 22;
    pub const LINK: u32 = 23;
    pub const LOCAL_LINK: u32 = 24;
    pub const ONLY_CHILD: u32 = 25;
    pub const ONLY_OF_TYPE: u32 = 26;
    pub const OPTIONAL: u32 = 27;
    pub const OUT_OF_RANGE: u32 = 28;
    pub const PAST: u32 = 29;
    pub const PLACEHOLDER_SHOWN: u32 = 30;
    pub const READ_ONLY: u32 = 31;
    pub const READ_WRITE: u32 = 32;
    pub const REQUIRED: u32 = 33;
    pub const ROOT: u32 = 34;
    pub const SCOPE: u32 = 35;
    pub const TARGET: u32 = 36;
    pub const TARGET_WITHIN: u32 = 37;
    pub const USER_INVALID: u32 = 38;
    pub const VALID: u32 = 39;
    pub const VISITED: u32 = 40;
    pub const WARNING: u32 = 41;
}

/// `lxb_css_selector_pseudo_class_function_id_t`.
pub mod class_function {
    pub const UNDEF: u32 = 0;
    pub const CURRENT: u32 = 1;
    pub const DIR: u32 = 2;
    pub const HAS: u32 = 3;
    pub const IS: u32 = 4;
    pub const LANG: u32 = 5;
    pub const LEXBOR_CONTAINS: u32 = 6;
    pub const NOT: u32 = 7;
    pub const NTH_CHILD: u32 = 8;
    pub const NTH_COL: u32 = 9;
    pub const NTH_LAST_CHILD: u32 = 10;
    pub const NTH_LAST_COL: u32 = 11;
    pub const NTH_LAST_OF_TYPE: u32 = 12;
    pub const NTH_OF_TYPE: u32 = 13;
    pub const WHERE: u32 = 14;
}

/// `lxb_css_selector_pseudo_element_id_t`.
pub mod element {
    pub const UNDEF: u32 = 0;
    pub const AFTER: u32 = 1;
    pub const BACKDROP: u32 = 2;
    pub const BEFORE: u32 = 3;
    pub const FIRST_LETTER: u32 = 4;
    pub const FIRST_LINE: u32 = 5;
    pub const GRAMMAR_ERROR: u32 = 6;
    pub const INACTIVE_SELECTION: u32 = 7;
    pub const MARKER: u32 = 8;
    pub const PLACEHOLDER: u32 = 9;
    pub const SELECTION: u32 = 10;
    pub const SPELLING_ERROR: u32 = 11;
    pub const TARGET_TEXT: u32 = 12;
}

/// `lxb_css_selector_pseudo_element_function_id_t`: only `#undef`.
pub mod element_function {
    pub const UNDEF: u32 = 0;
}

/// `lxb_css_selectors_pseudo_data_t`.
pub struct Data {
    pub name: &'static [u8],
    pub id: u32,
}

/// `lxb_css_selectors_pseudo_data_func_t`.
pub struct FuncData {
    pub name: &'static [u8],
    pub id: u32,
    pub empty: bool,
    pub combinator: u8,
    /// `cb.value`; `cb.failed` is `lxb_css_state_failed` for every entry.
    pub value: St,
    /// `cb.end`.
    pub end: EndCb,
    pub forgiving: bool,
    pub comma: bool,
}

macro_rules! data {
    ($($name:literal => $id:path),* $(,)?) => {
        &[$(Data { name: $name, id: $id }),*]
    };
}

/// `lxb_css_selectors_pseudo_data_pseudo_class`, `#undef` included.
pub static CLASSES: &[Data] = data![
    b"#undef" => class::UNDEF,
    b"active" => class::ACTIVE,
    b"any-link" => class::ANY_LINK,
    b"blank" => class::BLANK,
    b"checked" => class::CHECKED,
    b"current" => class::CURRENT,
    b"default" => class::DEFAULT,
    b"disabled" => class::DISABLED,
    b"empty" => class::EMPTY,
    b"enabled" => class::ENABLED,
    b"first-child" => class::FIRST_CHILD,
    b"first-of-type" => class::FIRST_OF_TYPE,
    b"focus" => class::FOCUS,
    b"focus-visible" => class::FOCUS_VISIBLE,
    b"focus-within" => class::FOCUS_WITHIN,
    b"fullscreen" => class::FULLSCREEN,
    b"future" => class::FUTURE,
    b"hover" => class::HOVER,
    b"in-range" => class::IN_RANGE,
    b"indeterminate" => class::INDETERMINATE,
    b"invalid" => class::INVALID,
    b"last-child" => class::LAST_CHILD,
    b"last-of-type" => class::LAST_OF_TYPE,
    b"link" => class::LINK,
    b"local-link" => class::LOCAL_LINK,
    b"only-child" => class::ONLY_CHILD,
    b"only-of-type" => class::ONLY_OF_TYPE,
    b"optional" => class::OPTIONAL,
    b"out-of-range" => class::OUT_OF_RANGE,
    b"past" => class::PAST,
    b"placeholder-shown" => class::PLACEHOLDER_SHOWN,
    b"read-only" => class::READ_ONLY,
    b"read-write" => class::READ_WRITE,
    b"required" => class::REQUIRED,
    b"root" => class::ROOT,
    b"scope" => class::SCOPE,
    b"target" => class::TARGET,
    b"target-within" => class::TARGET_WITHIN,
    b"user-invalid" => class::USER_INVALID,
    b"valid" => class::VALID,
    b"visited" => class::VISITED,
    b"warning" => class::WARNING,
];

/// `lxb_css_selectors_pseudo_data_pseudo_element`, `#undef` included.
pub static ELEMENTS: &[Data] = data![
    b"#undef" => element::UNDEF,
    b"after" => element::AFTER,
    b"backdrop" => element::BACKDROP,
    b"before" => element::BEFORE,
    b"first-letter" => element::FIRST_LETTER,
    b"first-line" => element::FIRST_LINE,
    b"grammar-error" => element::GRAMMAR_ERROR,
    b"inactive-selection" => element::INACTIVE_SELECTION,
    b"marker" => element::MARKER,
    b"placeholder" => element::PLACEHOLDER,
    b"selection" => element::SELECTION,
    b"spelling-error" => element::SPELLING_ERROR,
    b"target-text" => element::TARGET_TEXT,
];

const fn func(
    name: &'static [u8],
    id: u32,
    combinator: u8,
    value: St,
    end: EndCb,
    forgiving: bool,
    comma: bool,
) -> FuncData {
    FuncData {
        name,
        id,
        empty: false,
        combinator,
        value,
        end,
        forgiving,
        comma,
    }
}

/// `lxb_css_selectors_pseudo_data_pseudo_class_function`, `#undef` included.
pub static CLASS_FUNCTIONS: &[FuncData] = &[
    func(
        b"#undef",
        class_function::UNDEF,
        comb::CLOSE,
        St::PseudoClassFunctionUndef,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"current",
        class_function::CURRENT,
        comb::CLOSE,
        St::PseudoClassFunctionList,
        EndCb::FunctionEnd,
        false,
        true,
    ),
    func(
        b"dir",
        class_function::DIR,
        comb::CLOSE,
        St::PseudoClassFunctionUnsupported,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"has",
        class_function::HAS,
        comb::DESCENDANT,
        St::PseudoClassFunctionHas,
        EndCb::FunctionForgivingRelative,
        true,
        true,
    ),
    func(
        b"is",
        class_function::IS,
        comb::CLOSE,
        St::PseudoClassFunctionList,
        EndCb::FunctionForgiving,
        true,
        true,
    ),
    func(
        b"lang",
        class_function::LANG,
        comb::CLOSE,
        St::PseudoClassFunctionUnsupported,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"lexbor-contains",
        class_function::LEXBOR_CONTAINS,
        comb::CLOSE,
        St::PseudoClassFunctionUnsupported,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"not",
        class_function::NOT,
        comb::CLOSE,
        St::PseudoClassFunctionList,
        EndCb::FunctionEnd,
        false,
        true,
    ),
    func(
        b"nth-child",
        class_function::NTH_CHILD,
        comb::CLOSE,
        St::PseudoOfBegin,
        EndCb::FunctionEnd,
        false,
        true,
    ),
    func(
        b"nth-col",
        class_function::NTH_COL,
        comb::CLOSE,
        St::PseudoAnbBegin,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"nth-last-child",
        class_function::NTH_LAST_CHILD,
        comb::CLOSE,
        St::PseudoOfBegin,
        EndCb::FunctionEnd,
        false,
        true,
    ),
    func(
        b"nth-last-col",
        class_function::NTH_LAST_COL,
        comb::CLOSE,
        St::PseudoAnbBegin,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"nth-last-of-type",
        class_function::NTH_LAST_OF_TYPE,
        comb::CLOSE,
        St::PseudoAnbBegin,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"nth-of-type",
        class_function::NTH_OF_TYPE,
        comb::CLOSE,
        St::PseudoAnbBegin,
        EndCb::FunctionEnd,
        false,
        false,
    ),
    func(
        b"where",
        class_function::WHERE,
        comb::CLOSE,
        St::PseudoClassFunctionList,
        EndCb::FunctionForgiving,
        true,
        true,
    ),
];

/// `lxb_css_selectors_pseudo_data_pseudo_element_function`: `#undef` only.
pub static ELEMENT_FUNCTIONS: &[FuncData] = &[func(
    b"#undef",
    element_function::UNDEF,
    comb::CLOSE,
    St::PseudoElementFunctionUndef,
    EndCb::FunctionEnd,
    false,
    false,
)];

/// The `#undef` entry is not in Lexbor's name tables.
fn by_name<'t, T>(table: &'t [T], name_of: impl Fn(&T) -> &[u8], name: &[u8]) -> Option<&'t T> {
    table
        .iter()
        .skip(1)
        .find(|d| name_of(d).eq_ignore_ascii_case(name))
}

/// `lxb_css_selector_pseudo_class_by_name`.
pub fn class_by_name(name: &[u8]) -> Option<&'static Data> {
    by_name(CLASSES, |d| d.name, name)
}

/// `lxb_css_selector_pseudo_class_function_by_name`.
pub fn class_function_by_name(name: &[u8]) -> Option<&'static FuncData> {
    by_name(CLASS_FUNCTIONS, |d| d.name, name).filter(|d| d.id != class_function::LEXBOR_CONTAINS)
}

/// `lxb_css_selector_pseudo_element_by_name`.
pub fn element_by_name(name: &[u8]) -> Option<&'static Data> {
    by_name(ELEMENTS, |d| d.name, name)
}

/// `lxb_css_selector_pseudo_element_function_by_name`.
pub fn element_function_by_name(name: &[u8]) -> Option<&'static FuncData> {
    by_name(ELEMENT_FUNCTIONS, |d| d.name, name)
}

/// `lxb_css_selector_pseudo_function_by_id`. The ids a selector carries
/// come from the tables, so the lookup always lands.
pub fn function_by_id(id: u32, is_class: bool) -> &'static FuncData {
    let table = if is_class {
        CLASS_FUNCTIONS
    } else {
        ELEMENT_FUNCTIONS
    };
    table.get(id as usize).unwrap_or(&table[0])
}

/// `lxb_css_selector_pseudo_function_can_empty`.
pub fn function_can_empty(id: u32, is_class: bool) -> bool {
    function_by_id(id, is_class).empty
}
