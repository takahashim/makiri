//! The parsed selector: Lexbor's `lxb_css_selector_list_t` /
//! `lxb_css_selector_t` graph as an index arena, and the views the consumers
//! read it through.
//!
//! The arena keeps Lexbor's links (`first`/`last`/`next`/`prev`/`parent`) so
//! the parser states port line for line. Destroying a node only unlinks it:
//! the arena is dropped as a whole with the parse.
//!
//! The views - [`List`], [`Lists`], [`Selector`] and the enums - have the
//! shape `lexbor::css_parser`'s views over Lexbor's structs have, so a
//! consumer reads either the same way.

use super::token::Span;
use core::ffi::c_long;

pub type ListId = u32;
pub type SelId = u32;

/// `lxb_css_selector_type_t`.
pub mod sel_type {
    pub const UNDEF: u8 = 0;
    pub const ANY: u8 = 1;
    pub const ELEMENT: u8 = 2;
    pub const ID: u8 = 3;
    pub const CLASS: u8 = 4;
    pub const ATTRIBUTE: u8 = 5;
    pub const PSEUDO_CLASS: u8 = 6;
    pub const PSEUDO_CLASS_FUNCTION: u8 = 7;
    pub const PSEUDO_ELEMENT: u8 = 8;
    pub const PSEUDO_ELEMENT_FUNCTION: u8 = 9;
}

/// `lxb_css_selector_combinator_t`.
pub mod comb {
    pub const DESCENDANT: u8 = 0;
    pub const CLOSE: u8 = 1;
    pub const CHILD: u8 = 2;
    pub const SIBLING: u8 = 3;
    pub const FOLLOWING: u8 = 4;
    pub const CELL: u8 = 5;
}

/// `lxb_css_selector_match_t`.
pub mod matching {
    pub const EQUAL: u8 = 0;
    pub const INCLUDE: u8 = 1;
    pub const DASH: u8 = 2;
    pub const PREFIX: u8 = 3;
    pub const SUFFIX: u8 = 4;
    pub const SUBSTRING: u8 = 5;
}

/// `lxb_css_selector_modifier_t`.
pub mod modifier {
    pub const UNSET: u8 = 0;
    pub const I: u8 = 1;
    pub const S: u8 = 2;
}

/// `lxb_css_selector_anb_of_t`: `An+B` and the `of S` list.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnbOf {
    pub a: c_long,
    pub b: c_long,
    pub of: Option<ListId>,
}

/// `selector->u.pseudo.data`.
#[derive(Clone, Copy, Debug, Default)]
pub enum PseudoData {
    #[default]
    None,
    /// The argument list of `:is()` and its kind.
    List(ListId),
    /// The argument of the `:nth-*` family.
    AnbOf(AnbOf),
    /// The language ranges of `:lang()`, joined by NUL bytes (C34).
    Lang(Span),
}

/// `lxb_css_selector_list_t`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListNode {
    pub first: Option<SelId>,
    pub last: Option<SelId>,
    pub parent: Option<SelId>,
    pub next: Option<ListId>,
    pub prev: Option<ListId>,
    pub specificity: u32,
}

/// `lxb_css_selector_t`. Lexbor's `u` union is its two members side by side:
/// an attribute selector writes `attr_match`/`modifier`/`value`, a pseudo
/// selector `pseudo_type`/`data`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SelNode {
    pub ty: u8,
    pub combinator: u8,
    pub name: Option<Span>,
    pub ns: Option<Span>,
    pub attr_match: u8,
    pub modifier: u8,
    pub value: Option<Span>,
    pub pseudo_type: u32,
    pub data: PseudoData,
    pub next: Option<SelId>,
    pub prev: Option<SelId>,
    pub list: ListId,
}

/// The arena one parse builds.
#[derive(Default)]
pub struct Ast {
    pub lists: Vec<ListNode>,
    pub sels: Vec<SelNode>,
    pub strings: Vec<u8>,
}

impl Ast {
    #[inline]
    pub fn list(&self, id: ListId) -> &ListNode {
        &self.lists[id as usize]
    }

    #[inline]
    pub fn list_mut(&mut self, id: ListId) -> &mut ListNode {
        &mut self.lists[id as usize]
    }

    #[inline]
    pub fn sel(&self, id: SelId) -> &SelNode {
        &self.sels[id as usize]
    }

    #[inline]
    pub fn sel_mut(&mut self, id: SelId) -> &mut SelNode {
        &mut self.sels[id as usize]
    }

    pub fn str(&self, s: Span) -> &[u8] {
        let start = s.start as usize;
        self.strings
            .get(start..start + s.len as usize)
            .unwrap_or(&[])
    }

    /// The comma groups of the list starting at `first`.
    pub fn lists_from(&self, first: Option<ListId>) -> Lists<'_> {
        Lists(first.map(|id| List { ast: self, id }))
    }
}

/* ---- the views ---- */

/// A selector list: a comma group of the query, or the argument of a
/// functional pseudo-class.
#[derive(Clone, Copy)]
pub struct List<'p> {
    ast: &'p Ast,
    id: ListId,
}

impl<'p> List<'p> {
    /// The first simple selector of the list's chain.
    pub fn first(self) -> Option<Selector<'p>> {
        self.ast
            .list(self.id)
            .first
            .map(|id| Selector { ast: self.ast, id })
    }

    /// Lexbor's packed specificity for this list.
    pub fn specificity(self) -> u32 {
        self.ast.list(self.id).specificity
    }
}

/// Selector lists followed through their `next` links.
#[derive(Clone, Copy)]
pub struct Lists<'p>(Option<List<'p>>);

impl<'p> Iterator for Lists<'p> {
    type Item = List<'p>;

    fn next(&mut self) -> Option<List<'p>> {
        let cur = self.0?;
        self.0 = cur
            .ast
            .list(cur.id)
            .next
            .map(|id| List { ast: cur.ast, id });
        Some(cur)
    }
}

/// One simple selector of a chain.
#[derive(Clone, Copy)]
pub struct Selector<'p> {
    ast: &'p Ast,
    id: SelId,
}

/// Identity: two views of the same selector.
impl PartialEq for Selector<'_> {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.ast, other.ast) && self.id == other.id
    }
}

/// How a simple selector attaches to the one before it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    /// `||`, the column combinator.
    Column,
    Other,
}

/// An attribute selector's operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttrMatch {
    Equal,
    Include,
    Dash,
    Prefix,
    Suffix,
    Substring,
    Other,
}

/// An attribute selector's operator, case modifier and value.
#[derive(Clone, Copy, Debug)]
pub struct Attribute<'p> {
    pub op: AttrMatch,
    pub case: CaseModifier,
    /// None for `[name]`, an existence test.
    pub value: Option<&'p [u8]>,
}

/// The case modifier written after an attribute selector's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaseModifier {
    Unset,
    Insensitive,
    Sensitive,
}

/// The non-functional pseudo-classes the consumers tell apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    Empty,
    Root,
    FirstOfType,
    LastOfType,
    OnlyOfType,
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
    PlaceholderShown,
    Scope,
    Defined,
    Other,
}

/// `:nth-*(an+b [of S])`.
#[derive(Clone, Copy)]
pub struct Nth<'p> {
    pub a: c_long,
    pub b: c_long,
    /// The `S` of an `of S` clause, `None` when none was written.
    pub of: Option<Lists<'p>>,
}

/// The functional pseudo-classes that take a selector list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListPseudo {
    Not,
    Is,
    Where,
    Has,
}

/// A functional pseudo-class, with the argument its id says was stored.
#[derive(Clone, Copy)]
pub enum FunctionArg<'p> {
    Nth {
        from_end: bool,
        of_type: bool,
        anb: Option<Nth<'p>>,
    },
    Selectors {
        pseudo: ListPseudo,
        lists: Lists<'p>,
    },
    /// `:lexbor-contains()`: never produced, the port does not know the name.
    Contains,
    Current(Lists<'p>),
    /// `:lang()`'s language ranges.
    Lang(LangRanges<'p>),
    Other,
}

/// The language ranges of a `:lang()`, in the order written.
#[derive(Clone, Copy)]
pub struct LangRanges<'p>(&'p [u8]);

impl<'p> LangRanges<'p> {
    pub fn iter(self) -> impl Iterator<Item = &'p [u8]> {
        self.0.split(|&b| b == 0)
    }
}

/// What a simple selector is, with the parts its kind carries.
#[derive(Clone, Copy)]
pub enum Simple<'p> {
    Universal,
    Type,
    Id,
    Class,
    Attribute(Attribute<'p>),
    PseudoClass(PseudoClass),
    PseudoClassFunction(FunctionArg<'p>),
    PseudoElement,
    Other,
}

impl<'p> Selector<'p> {
    fn node(self) -> &'p SelNode {
        self.ast.sel(self.id)
    }

    /// The kind of this simple selector, with its kind's parts.
    pub fn simple(self) -> Simple<'p> {
        match self.node().ty {
            sel_type::ANY => Simple::Universal,
            sel_type::ELEMENT => Simple::Type,
            sel_type::ID => Simple::Id,
            sel_type::CLASS => Simple::Class,
            sel_type::ATTRIBUTE => Simple::Attribute(self.attribute()),
            sel_type::PSEUDO_CLASS => Simple::PseudoClass(self.pseudo_class()),
            sel_type::PSEUDO_CLASS_FUNCTION => Simple::PseudoClassFunction(self.function_arg()),
            sel_type::PSEUDO_ELEMENT | sel_type::PSEUDO_ELEMENT_FUNCTION => Simple::PseudoElement,
            _ => Simple::Other,
        }
    }

    pub fn combinator(self) -> Combinator {
        match self.node().combinator {
            comb::DESCENDANT => Combinator::Descendant,
            comb::CLOSE => Combinator::Close,
            comb::CHILD => Combinator::Child,
            comb::SIBLING => Combinator::NextSibling,
            comb::FOLLOWING => Combinator::SubsequentSibling,
            comb::CELL => Combinator::Column,
            _ => Combinator::Other,
        }
    }

    /// The name, empty when there is none.
    pub fn name(self) -> &'p [u8] {
        self.node().name.map_or(&[], |s| self.ast.str(s))
    }

    /// The namespace as written: None with no pipe, empty for `|name`.
    pub fn ns(self) -> Option<&'p [u8]> {
        self.node().ns.map(|s| self.ast.str(s))
    }

    /// The next simple selector of the chain.
    pub fn next(self) -> Option<Selector<'p>> {
        self.node().next.map(|id| Selector { ast: self.ast, id })
    }

    /// `::x()` rather than `::x`.
    pub fn is_pseudo_element_function(self) -> bool {
        self.node().ty == sel_type::PSEUDO_ELEMENT_FUNCTION
    }

    fn attribute(self) -> Attribute<'p> {
        let n = self.node();
        Attribute {
            op: match n.attr_match {
                matching::EQUAL => AttrMatch::Equal,
                matching::INCLUDE => AttrMatch::Include,
                matching::DASH => AttrMatch::Dash,
                matching::PREFIX => AttrMatch::Prefix,
                matching::SUFFIX => AttrMatch::Suffix,
                matching::SUBSTRING => AttrMatch::Substring,
                _ => AttrMatch::Other,
            },
            case: match n.modifier {
                modifier::UNSET => CaseModifier::Unset,
                modifier::S => CaseModifier::Sensitive,
                _ => CaseModifier::Insensitive,
            },
            value: n.value.map(|s| self.ast.str(s)),
        }
    }

    fn pseudo_class(self) -> PseudoClass {
        use super::pseudo::class as pc;
        match self.node().pseudo_type {
            pc::FIRST_CHILD => PseudoClass::FirstChild,
            pc::LAST_CHILD => PseudoClass::LastChild,
            pc::ONLY_CHILD => PseudoClass::OnlyChild,
            pc::EMPTY => PseudoClass::Empty,
            pc::ROOT => PseudoClass::Root,
            pc::FIRST_OF_TYPE => PseudoClass::FirstOfType,
            pc::LAST_OF_TYPE => PseudoClass::LastOfType,
            pc::ONLY_OF_TYPE => PseudoClass::OnlyOfType,
            pc::ANY_LINK => PseudoClass::AnyLink,
            pc::LINK => PseudoClass::Link,
            pc::BLANK => PseudoClass::Blank,
            pc::CHECKED => PseudoClass::Checked,
            pc::DISABLED => PseudoClass::Disabled,
            pc::ENABLED => PseudoClass::Enabled,
            pc::OPTIONAL => PseudoClass::Optional,
            pc::REQUIRED => PseudoClass::Required,
            pc::READ_ONLY => PseudoClass::ReadOnly,
            pc::READ_WRITE => PseudoClass::ReadWrite,
            pc::ACTIVE => PseudoClass::Active,
            pc::FOCUS => PseudoClass::Focus,
            pc::HOVER => PseudoClass::Hover,
            pc::PLACEHOLDER_SHOWN => PseudoClass::PlaceholderShown,
            pc::SCOPE => PseudoClass::Scope,
            pc::DEFINED => PseudoClass::Defined,
            _ => PseudoClass::Other,
        }
    }

    fn function_arg(self) -> FunctionArg<'p> {
        use super::pseudo::class_function as pf;
        let n = self.node();
        let ast = self.ast;
        let nth = |from_end: bool, of_type: bool| FunctionArg::Nth {
            from_end,
            of_type,
            anb: match n.data {
                PseudoData::AnbOf(x) => Some(Nth {
                    a: x.a,
                    b: x.b,
                    of: x.of.map(|id| ast.lists_from(Some(id))),
                }),
                _ => None,
            },
        };
        let lists = || match n.data {
            PseudoData::List(id) => ast.lists_from(Some(id)),
            _ => ast.lists_from(None),
        };
        let selectors = |pseudo: ListPseudo| FunctionArg::Selectors {
            pseudo,
            lists: lists(),
        };
        match n.pseudo_type {
            pf::NTH_CHILD => nth(false, false),
            pf::NTH_LAST_CHILD => nth(true, false),
            pf::NTH_OF_TYPE => nth(false, true),
            pf::NTH_LAST_OF_TYPE => nth(true, true),
            pf::NOT => selectors(ListPseudo::Not),
            pf::IS => selectors(ListPseudo::Is),
            pf::WHERE => selectors(ListPseudo::Where),
            pf::HAS => selectors(ListPseudo::Has),
            pf::LEXBOR_CONTAINS => FunctionArg::Contains,
            pf::CURRENT => FunctionArg::Current(lists()),
            pf::LANG => match n.data {
                PseudoData::Lang(span) => FunctionArg::Lang(LangRanges(ast.str(span))),
                _ => FunctionArg::Other,
            },
            _ => FunctionArg::Other,
        }
    }
}
