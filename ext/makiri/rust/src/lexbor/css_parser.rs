//! The process-global Lexbor CSS parser - selector parsing only, NOT the
//! matcher.
//!
//! CSS compilation runs under the GVL (the glue holds it throughout), so a
//! single global is safe with no locking, the same argument the HTML CSS engine
//! makes - and [`parse`] takes the [`Gvl`] that says so. Created lazily; a creation failure is reported to the caller rather
//! than retried, and leaves the globals untouched so a later call tries again.
//!
//! # The arena is cleaned on every path out
//!
//! The parsed selector list lives in the parser's memory arena, which is reset
//! between calls. That is a `Drop`, so a path added later cannot forget - and
//! the lowering borrows the list, so the reset must not happen before the
//! lowering is done.

#![allow(unsafe_code)]

//! # Unsafe boundary
//!
//! Lexbor owns this parser and exposes it as three raw pointers. This module is
//! the only CSS module allowed to retain those pointers or mutate the
//! process-global parser state, which it reaches through a [`GvlCell`].
//!
//! It is also the only reader of what a parse builds: the lowering walks the
//! selectors through [`List`] and [`Selector`], borrowed from [`Parsed`], so
//! no pointer into the arena outlives the parse and every union read is keyed
//! by the tag Lexbor stored beside it.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use crate::gvl::{Gvl, GvlCell, GvlRef};
use crate::lexbor::abi as lxb;
use crate::lexbor::css_engine::{lexbor_str, ParseFail, ParserParts, SelectorParser};
use crate::text::VerifiedText;
use core::ffi::c_long;

/// `lxb_css_selector_list_t`, generated.
type SelectorList = lxb::lxb_css_selector_list_t;
type RawSelector = lxb::lxb_css_selector_t;

/// Why a parse did not produce a selector list.
pub enum ParseError {
    /// The parser could not be created.
    NotReady,
    /// The selector is malformed.
    Syntax,
    /// A parse on this thread is still borrowing the parser.
    Busy,
    /// Out of memory parsing - not a verdict on the selector.
    Oom,
}

/// The process-global parser, built on first use. A build failure leaves it
/// unset, so a later call tries again.
static ENGINE: GvlCell<Option<SelectorParser>> = GvlCell::new(None);

/// A parsed selector list, borrowed from the engine's arena.
///
/// Dropping it cleans the arena and returns the parser to its CLEAN stage, which
/// is what makes the next call safe. The list must not be read afterwards - the
/// lifetime says so. It holds the borrow of the global for as long as it lives,
/// so a second parse cannot reuse the arena under it.
pub struct Parsed<'g> {
    first: *mut SelectorList,
    engine: SelectorParser,
    /// Released after `drop` below has cleaned the arena.
    _slot: GvlRef<'g, Option<SelectorParser>>,
}

impl Drop for Parsed<'_> {
    fn drop(&mut self) {
        // SAFETY: the borrow in `_slot` is live, and the list is not read after
        // `self` goes.
        unsafe { self.engine.clean_all() };
    }
}

/// Parse `selector` into the engine's arena.
pub fn parse<'g>(gvl: &'g Gvl, selector: VerifiedText) -> Result<Parsed<'g>, ParseError> {
    let mut slot = ENGINE.borrow(gvl).map_err(|_| ParseError::Busy)?;
    let e = match *slot {
        Some(e) => e,
        None => *slot.insert(
            ParserParts::build()
                .ok_or(ParseError::NotReady)?
                .into_parser(),
        ),
    };

    // SAFETY: the borrow in `slot` is live; the verified slice is live for the
    // call.
    let list = unsafe { e.parse(selector.as_bytes()) };
    let parsed = Parsed {
        first: list.unwrap_or(core::ptr::null_mut()),
        engine: e,
        _slot: slot,
    };
    match list {
        Ok(_) => Ok(parsed),
        /* `parsed` drops on both: the arena is cleaned. */
        Err(ParseFail::Rejected) => Err(ParseError::Syntax),
        Err(ParseFail::GuardOom | ParseFail::ParserOom) => Err(ParseError::Oom),
    }
}

/* ------------------------------------------------------------------ *
 * the parse, read through typed views                                *
 * ------------------------------------------------------------------ */

/// Lexbor's generated enum values. Nothing outside this module sees them: the
/// views below translate each into a Rust enum, so a value the lowering does
/// not handle reaches it as an explicit `Other` - never as a number that a
/// catch-all arm could mistake for one it does handle.
mod raw {
    use crate::lexbor::abi as l;

    type Ty = l::lxb_css_selector_type_t;
    pub const ANY: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ANY;
    pub const ELEMENT: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ELEMENT;
    pub const ID: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ID;
    pub const CLASS: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_CLASS;
    pub const ATTRIBUTE: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_ATTRIBUTE;
    pub const PSEUDO_CLASS: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS;
    pub const PSEUDO_CLASS_FUNCTION: Ty =
        l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS_FUNCTION;
    pub const PSEUDO_ELEMENT: Ty = l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT;
    pub const PSEUDO_ELEMENT_FUNCTION: Ty =
        l::lxb_css_selector_type_t_LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT_FUNCTION;

    type Comb = l::lxb_css_selector_combinator_t;
    pub const DESCENDANT: Comb =
        l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_DESCENDANT;
    pub const CLOSE: Comb = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_CLOSE;
    pub const CHILD: Comb = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_CHILD;
    pub const SIBLING: Comb = l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_SIBLING;
    pub const FOLLOWING: Comb =
        l::lxb_css_selector_combinator_t_LXB_CSS_SELECTOR_COMBINATOR_FOLLOWING;

    type Match = l::lxb_css_selector_match_t;
    pub const EQUAL: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_EQUAL;
    pub const INCLUDE: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_INCLUDE;
    pub const DASH: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_DASH;
    pub const PREFIX: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_PREFIX;
    pub const SUFFIX: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_SUFFIX;
    pub const SUBSTRING: Match = l::lxb_css_selector_match_t_LXB_CSS_SELECTOR_MATCH_SUBSTRING;

    pub const MOD_UNSET: l::lxb_css_selector_modifier_t =
        l::lxb_css_selector_modifier_t_LXB_CSS_SELECTOR_MODIFIER_UNSET;
    pub const MOD_S: l::lxb_css_selector_modifier_t =
        l::lxb_css_selector_modifier_t_LXB_CSS_SELECTOR_MODIFIER_S;

    type Pc = l::lxb_css_selector_pseudo_class_id_t;
    pub const FIRST_CHILD: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_CHILD;
    pub const LAST_CHILD: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_CHILD;
    pub const ONLY_CHILD: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_CHILD;
    pub const EMPTY: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_EMPTY;
    pub const ROOT: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ROOT;
    pub const FIRST_OF_TYPE: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_OF_TYPE;
    pub const LAST_OF_TYPE: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_OF_TYPE;
    pub const ONLY_OF_TYPE: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_OF_TYPE;
    // The rest are for `lexbor::selector_port` (HTML matching), not the XML
    // lowering, which treats all of them as unsupported (`PseudoClass::Other`).
    pub const ANY_LINK: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ANY_LINK;
    pub const LINK: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_LINK;
    pub const BLANK: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_BLANK;
    pub const CHECKED: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_CHECKED;
    pub const DISABLED: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_DISABLED;
    pub const ENABLED: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ENABLED;
    pub const OPTIONAL: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_OPTIONAL;
    pub const REQUIRED: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_REQUIRED;
    pub const READ_ONLY: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_READ_ONLY;
    pub const READ_WRITE: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_READ_WRITE;
    pub const ACTIVE: Pc =
        l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_ACTIVE;
    pub const FOCUS: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FOCUS;
    pub const HOVER: Pc = l::lxb_css_selector_pseudo_class_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_HOVER;

    type Pf = l::lxb_css_selector_pseudo_class_function_id_t;
    pub const NTH_CHILD: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_CHILD;
    pub const NTH_LAST_CHILD: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_CHILD;
    pub const NTH_OF_TYPE: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_OF_TYPE;
    pub const NTH_LAST_OF_TYPE: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_OF_TYPE;
    pub const NOT: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NOT;
    pub const IS: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_IS;
    pub const WHERE: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_WHERE;
    pub const HAS: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_HAS;
    pub const LEXBOR_CONTAINS: Pf =
        l::lxb_css_selector_pseudo_class_function_id_t_LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_LEXBOR_CONTAINS;
}

/// A reference into the arena a parse built.
///
/// # Safety
/// `p` must be null or point into the arena of a [`Parsed`] that lives for
/// `'p`.
unsafe fn arena<'p, T>(p: *const T) -> Option<&'p T> {
    // SAFETY: forwarded.
    unsafe { p.as_ref() }
}

impl Parsed<'_> {
    /// The selector's comma groups, in order.
    pub fn groups(&self) -> Lists<'_> {
        // SAFETY: `first` is null or the list Lexbor built, which the arena
        // keeps until `self` drops.
        Lists(unsafe { arena(self.first) }.map(List))
    }
}

/// A selector list: a comma group of the query, or the argument of a
/// functional pseudo-class.
#[derive(Clone, Copy)]
pub struct List<'p>(&'p SelectorList);

impl<'p> List<'p> {
    /// The first simple selector of the list's chain.
    pub fn first(self) -> Option<Selector<'p>> {
        // SAFETY: a list's links point into its own arena.
        unsafe { arena(self.0.first) }.map(Selector)
    }
}

/// Selector lists followed through their `next` links.
#[derive(Clone, Copy)]
pub struct Lists<'p>(Option<List<'p>>);

impl<'p> Iterator for Lists<'p> {
    type Item = List<'p>;

    fn next(&mut self) -> Option<List<'p>> {
        let cur = self.0?;
        // SAFETY: as in `List::first`.
        self.0 = unsafe { arena(cur.0.next) }.map(List);
        Some(cur)
    }
}

/// One simple selector of a chain.
#[derive(Clone, Copy)]
pub struct Selector<'p>(&'p RawSelector);

/// Identity: two views of the same selector.
impl PartialEq for Selector<'_> {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0, other.0)
    }
}

/// How a simple selector attaches to the one before it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// Whitespace.
    Descendant,
    /// None written: the selector continues the same compound (`a.b`).
    Close,
    /// `>`
    Child,
    /// `+`
    NextSibling,
    /// `~`
    SubsequentSibling,
    /// Anything else Lexbor parses - the column combinator `||`.
    Other,
}

/// An attribute selector's operator.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttrMatch {
    /// `=`
    Equal,
    /// `~=`
    Include,
    /// `|=`
    Dash,
    /// `^=`
    Prefix,
    /// `$=`
    Suffix,
    /// `*=`
    Substring,
    Other,
}

/// An attribute selector's operator, case modifier and value.
#[derive(Clone, Copy)]
pub struct Attribute<'p> {
    pub op: AttrMatch,
    /// Whether an `i` modifier was written: match the value ASCII
    /// case-insensitively. `s`, case-sensitive, is how values compare anyway,
    /// so it reads as no modifier - for the XML lowering, the sole consumer
    /// when this was added, XML attribute values are always case-sensitive
    /// regardless, so `s` and unset need no distinguishing there.
    pub case_insensitive: bool,
    /// Whether an explicit `s` modifier was written, as opposed to no
    /// modifier at all - a distinction `case_insensitive` alone cannot make
    /// (both read `false`), needed by `lexbor::selector_port`'s HTML
    /// case-insensitive-attribute-value table (§B-4/B-5 in
    /// `notes/lexbor_selectors_c_semantics.ja.md`): an explicit `s` forces
    /// case-sensitive even for a table attribute like `type`, but no
    /// modifier at all defers to the table's default.
    pub explicit_sensitive: bool,
    /// None for `[name]`, an existence test.
    pub value: Option<&'p [u8]>,
}

/// The non-functional pseudo-classes the lowering can express.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    Empty,
    Root,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    // The rest are for `lexbor::selector_port` (HTML matching) only; the XML
    // lowering's exhaustive match (`css::lower::lower_pseudo_simple`) groups
    // them with `Other` - unsupported, same as before this was added.
    AnyLink,
    Link,
    Blank,
    Checked,
    Disabled,
    Enabled,
    Optional,
    Required,
    ReadOnly,
    ReadWrite,
    Active,
    Focus,
    Hover,
    Other,
}

/// `:nth-*(an+b [of S])`.
#[derive(Clone, Copy)]
pub struct Nth<'p> {
    pub a: c_long,
    pub b: c_long,
    /// Whether an `of S` clause was written. The XML lowering only needs to
    /// know that much (it refuses `of S` outright); `of_list` is the actual
    /// clause, for `lexbor::selector_port`'s HTML matching.
    pub of: bool,
    /// The `S` in `of S`, or `None` when no clause was written (same
    /// condition as `of`, kept separate rather than folding `of` into
    /// `of_list.is_some()` so the XML lowering's check reads as a plain bool).
    pub of_list: Option<Lists<'p>>,
}

/// `:lexbor-contains(needle [i])`.
#[derive(Clone, Copy)]
pub struct Contains<'p> {
    pub needle: &'p [u8],
    pub insensitive: bool,
}

/// The functional pseudo-classes that take a selector list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListPseudo {
    Not,
    Is,
    Where,
    Has,
}

/// A functional pseudo-class, with the argument its id says Lexbor stored.
///
/// The id is resolved HERE, once: each variant carries what tells its members
/// apart, so the lowering never reads the id a second time.
#[derive(Clone, Copy)]
pub enum FunctionArg<'p> {
    /// The `:nth-*` family; `anb` is None when Lexbor stored nothing.
    Nth {
        /// `:nth-last-*`: counted from the end.
        from_end: bool,
        /// `:nth-*-of-type`: counted among same-type siblings.
        of_type: bool,
        anb: Option<Nth<'p>>,
    },
    /// `:not`, `:is`, `:where` and `:has`.
    Selectors {
        pseudo: ListPseudo,
        lists: Lists<'p>,
    },
    /// `:lexbor-contains`, or None when Lexbor stored nothing.
    Contains(Option<Contains<'p>>),
    /// Any other functional pseudo-class.
    Other,
}

/// What a simple selector is, with the parts its kind carries.
#[derive(Clone, Copy)]
pub enum Simple<'p> {
    /// `*` or `ns|*`.
    Universal,
    /// A type selector: `el`, `ns|el`, `|el` or `*|el`.
    Type,
    /// `#id`
    Id,
    /// `.class`
    Class,
    Attribute(Attribute<'p>),
    PseudoClass(PseudoClass),
    PseudoClassFunction(FunctionArg<'p>),
    /// `::x` or `::x()`.
    PseudoElement,
    Other,
}

impl<'p> Selector<'p> {
    /// The kind of this simple selector, with its kind's parts.
    pub fn simple(self) -> Simple<'p> {
        match self.0.type_ {
            raw::ANY => Simple::Universal,
            raw::ELEMENT => Simple::Type,
            raw::ID => Simple::Id,
            raw::CLASS => Simple::Class,
            raw::ATTRIBUTE => Simple::Attribute(self.attribute()),
            raw::PSEUDO_CLASS => Simple::PseudoClass(self.pseudo_class()),
            raw::PSEUDO_CLASS_FUNCTION => Simple::PseudoClassFunction(self.function_arg()),
            raw::PSEUDO_ELEMENT | raw::PSEUDO_ELEMENT_FUNCTION => Simple::PseudoElement,
            _ => Simple::Other,
        }
    }

    pub fn combinator(self) -> Combinator {
        match self.0.combinator {
            raw::DESCENDANT => Combinator::Descendant,
            raw::CLOSE => Combinator::Close,
            raw::CHILD => Combinator::Child,
            raw::SIBLING => Combinator::NextSibling,
            raw::FOLLOWING => Combinator::SubsequentSibling,
            _ => Combinator::Other,
        }
    }

    /// The name, empty when there is none.
    pub fn name(self) -> &'p [u8] {
        // SAFETY: a selector's strings live in its arena.
        unsafe { lexbor_str(&self.0.name) }.unwrap_or(&[])
    }

    /// The namespace as written: None with no pipe, empty for `|name`.
    pub fn ns(self) -> Option<&'p [u8]> {
        // SAFETY: as in `name`.
        unsafe { lexbor_str(&self.0.ns) }
    }

    /// The next simple selector of the chain.
    pub fn next(self) -> Option<Selector<'p>> {
        // SAFETY: a selector's links point into its own arena.
        unsafe { arena(self.0.next) }.map(Selector)
    }

    /// An attribute selector's parts; `simple` calls it for no other kind.
    fn attribute(self) -> Attribute<'p> {
        // SAFETY: Lexbor fills `u.attribute` for an attribute selector.
        let at = unsafe { &self.0.u.attribute };
        Attribute {
            op: match at.match_ {
                raw::EQUAL => AttrMatch::Equal,
                raw::INCLUDE => AttrMatch::Include,
                raw::DASH => AttrMatch::Dash,
                raw::PREFIX => AttrMatch::Prefix,
                raw::SUFFIX => AttrMatch::Suffix,
                raw::SUBSTRING => AttrMatch::Substring,
                _ => AttrMatch::Other,
            },
            case_insensitive: at.modifier != raw::MOD_UNSET && at.modifier != raw::MOD_S,
            explicit_sensitive: at.modifier == raw::MOD_S,
            // SAFETY: as in `name`.
            value: unsafe { lexbor_str(&at.value) },
        }
    }

    /// A plain pseudo-class; `simple` calls it for no other kind.
    fn pseudo_class(self) -> PseudoClass {
        // SAFETY: Lexbor fills `u.pseudo` for a pseudo-class.
        match unsafe { self.0.u.pseudo.type_ } {
            raw::FIRST_CHILD => PseudoClass::FirstChild,
            raw::LAST_CHILD => PseudoClass::LastChild,
            raw::ONLY_CHILD => PseudoClass::OnlyChild,
            raw::EMPTY => PseudoClass::Empty,
            raw::ROOT => PseudoClass::Root,
            raw::FIRST_OF_TYPE => PseudoClass::FirstOfType,
            raw::LAST_OF_TYPE => PseudoClass::LastOfType,
            raw::ONLY_OF_TYPE => PseudoClass::OnlyOfType,
            raw::ANY_LINK => PseudoClass::AnyLink,
            raw::LINK => PseudoClass::Link,
            raw::BLANK => PseudoClass::Blank,
            raw::CHECKED => PseudoClass::Checked,
            raw::DISABLED => PseudoClass::Disabled,
            raw::ENABLED => PseudoClass::Enabled,
            raw::OPTIONAL => PseudoClass::Optional,
            raw::REQUIRED => PseudoClass::Required,
            raw::READ_ONLY => PseudoClass::ReadOnly,
            raw::READ_WRITE => PseudoClass::ReadWrite,
            raw::ACTIVE => PseudoClass::Active,
            raw::FOCUS => PseudoClass::Focus,
            raw::HOVER => PseudoClass::Hover,
            _ => PseudoClass::Other,
        }
    }

    /// A functional pseudo-class and its argument; `simple` calls it for no
    /// other kind.
    fn function_arg(self) -> FunctionArg<'p> {
        // SAFETY: Lexbor fills `u.pseudo` for a functional pseudo-class.
        let pseudo = unsafe { self.0.u.pseudo };
        let data = pseudo.data as *const core::ffi::c_void;
        let nth = |from_end: bool, of_type: bool| {
            // SAFETY: for the `:nth-*` family Lexbor stores an
            // `lxb_css_selector_anb_of_t`, in the arena.
            let anb = unsafe { arena(data as *const lxb::lxb_css_selector_anb_of_t) };
            FunctionArg::Nth {
                from_end,
                of_type,
                anb: anb.map(|n| Nth {
                    a: n.anb.a,
                    b: n.anb.b,
                    of: !n.of.is_null(),
                    // SAFETY: as in `name`/other `arena` calls - the list, if
                    // any, lives in the same arena.
                    of_list: unsafe { arena(n.of as *const SelectorList) }
                        .map(|r| Lists(Some(List(r)))),
                }),
            }
        };
        let selectors = |pseudo: ListPseudo| FunctionArg::Selectors {
            pseudo,
            // SAFETY: for these Lexbor stores a selector list, in the arena.
            lists: Lists(unsafe { arena(data as *const SelectorList) }.map(List)),
        };
        match pseudo.type_ {
            raw::NTH_CHILD => nth(false, false),
            raw::NTH_LAST_CHILD => nth(true, false),
            raw::NTH_OF_TYPE => nth(false, true),
            raw::NTH_LAST_OF_TYPE => nth(true, true),
            raw::NOT => selectors(ListPseudo::Not),
            raw::IS => selectors(ListPseudo::Is),
            raw::WHERE => selectors(ListPseudo::Where),
            raw::HAS => selectors(ListPseudo::Has),
            raw::LEXBOR_CONTAINS => {
                // SAFETY: Lexbor stores an `lxb_css_selector_contains_t`, in the
                // arena.
                let c = unsafe { arena(data as *const lxb::lxb_css_selector_contains_t) };
                FunctionArg::Contains(c.map(|c| Contains {
                    // SAFETY: as in `name`.
                    needle: unsafe { lexbor_str(&c.str_) }.unwrap_or(&[]),
                    insensitive: c.insensitive,
                }))
            }
            _ => FunctionArg::Other,
        }
    }
}
