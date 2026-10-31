//! The rules a stylesheet parse builds: Lexbor's `lxb_css_rule_*` structures
//! (`css/rule.h`), as far as the stylesheet reader reads them, in one index
//! arena. A rule's place in a list is Lexbor's `next` link; a selector list
//! is a list of the shared [`Ast`](super::ast::Ast), and a name is a span of
//! its string store. Every offset is a byte offset into the parsed text, as
//! Lexbor records it.

use super::ast::ListId;
use super::token::Span;

pub type ItemId = u32;

/// `lxb_css_at_rule_type_t`, the kinds a stylesheet can hold. `Undef` is
/// what Lexbor converts a typed at-rule into when its prelude or block is
/// not what the type allows (`lxb_css_at_rule_convert_to_undef`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtKind {
    Undef,
    Custom,
    FontFace,
    Media,
    Namespace,
}

/// One rule.
#[derive(Clone, Copy, Debug)]
pub enum Item {
    /// `lxb_css_rule_list_t`.
    List {
        first: Option<ItemId>,
        last: Option<ItemId>,
    },
    /// `lxb_css_rule_style_t`: the first declaration list of the block is
    /// its `declarations`, the rest of the block its `child`.
    Style {
        selector: Option<ListId>,
        prelude_begin: usize,
        prelude_end: usize,
        declarations: Option<ItemId>,
        child: Option<ItemId>,
    },
    /// `lxb_css_rule_bad_style_t`: a style rule whose selector failed.
    BadStyle {
        prelude_begin: usize,
        prelude_end: usize,
        declarations: Option<ItemId>,
        child: Option<ItemId>,
    },
    /// `lxb_css_rule_at_t`. `name` is the at-keyword's value.
    At {
        kind: AtKind,
        name: Span,
        prelude_begin: usize,
        prelude_end: usize,
        block: Option<ItemId>,
    },
    /// `lxb_css_rule_declaration_list_t`.
    DeclarationList {
        first: Option<ItemId>,
        last: Option<ItemId>,
    },
    /// `lxb_css_rule_declaration_t`: the name as the ident's value, the
    /// value as the offsets Lexbor records (`lxb_css_syntax_declaration_offset_t`).
    Declaration {
        name: Span,
        value_begin: usize,
        value_end: usize,
        important: bool,
    },
}

/// An item and its link to the next one in its list.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub item: Item,
    pub next: Option<ItemId>,
}

/// The arena: every rule a parse made, in creation order.
#[derive(Debug, Default)]
pub struct Rules {
    pub nodes: Vec<Node>,
}

impl Rules {
    #[inline]
    pub fn item(&self, id: ItemId) -> &Item {
        &self.nodes[id as usize].item
    }

    #[inline]
    pub fn item_mut(&mut self, id: ItemId) -> &mut Item {
        &mut self.nodes[id as usize].item
    }

    /// The items of a list (a rule list or a declaration list), in order.
    pub fn children(&self, list: Option<ItemId>) -> Children<'_> {
        let first = match list.map(|l| self.item(l)) {
            Some(Item::List { first, .. } | Item::DeclarationList { first, .. }) => *first,
            _ => None,
        };
        Children {
            rules: self,
            next: first,
        }
    }
}

pub struct Children<'a> {
    rules: &'a Rules,
    next: Option<ItemId>,
}

impl<'a> Iterator for Children<'a> {
    type Item = (ItemId, &'a Item);

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next?;
        let node = &self.rules.nodes[id as usize];
        self.next = node.next;
        Some((id, &node.item))
    }
}
