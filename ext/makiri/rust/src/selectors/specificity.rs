//! A selector's specificity as Selectors 4 calculates it (17, "Calculating
//! a selector's specificity"), replacing the packed value Lexbor's states
//! accumulate while they parse (docs/parser_differences.ja.md C23-C26).
//!
//! Lexbor counts a pseudo-class only when it takes an argument, counts a
//! prefixed type selector twice, caps each component at one inside a
//! functional pseudo-class instead of taking its most specific argument, and
//! lets a full component carry into the next. Here every complex selector
//! (a [`ListNode`](super::ast::ListNode)) is counted afresh once the parse is
//! done: A the IDs, B the classes, attributes and pseudo-classes, C the type
//! selectors and pseudo-elements; `:is()`, `:not()` and `:has()` count as
//! their most specific argument, `:nth-child()` and `:nth-last-child()` as
//! a pseudo-class plus their most specific `of S`, and `:where()` as
//! nothing. Each component is clamped at the most its nine bits hold.

use super::ast::{sel_type as st, Ast, ListId, PseudoData};
use super::pseudo::class_function as cf;

/// The largest value of one component.
const MAX: u32 = 0x1FF;

fn pack([a, b, c]: [u32; 3]) -> u32 {
    (a.min(MAX) << 18) | (b.min(MAX) << 9) | c.min(MAX)
}

/// `[a, b, c]` out of a packed value (a at 18, b at 9, c at 0, nine bits each).
pub(super) fn unpack(sp: u32) -> [u32; 3] {
    [(sp >> 18) & MAX, (sp >> 9) & MAX, sp & MAX]
}

/// The most specific of the lists chained from `first`, packed: a packed
/// value compares as the components do, A first.
fn most_specific(ast: &Ast, first: Option<ListId>) -> u32 {
    let mut best = 0;
    let mut at = first;
    while let Some(id) = at {
        let l = ast.list(id);
        best = best.max(l.specificity);
        at = l.next;
    }
    best
}

fn add(sum: &mut [u32; 3], sp: u32) {
    for (s, v) in sum.iter_mut().zip(unpack(sp)) {
        *s = s.saturating_add(v);
    }
}

/// Recount every list of `ast`. A functional pseudo-class's argument lists
/// are made after the list holding it, so walking the lists from the last
/// made counts every argument before the selector that uses it - without
/// recursion, however deep the nesting.
pub(crate) fn respecify(ast: &mut Ast) {
    for id in (0..ast.lists.len()).rev() {
        let mut sum = [0u32; 3];
        let mut at = ast.lists[id].first;
        while let Some(s) = at {
            let n = ast.sel(s);
            match n.ty {
                st::ID => sum[0] = sum[0].saturating_add(1),
                st::CLASS | st::ATTRIBUTE | st::PSEUDO_CLASS => {
                    sum[1] = sum[1].saturating_add(1);
                }
                st::ELEMENT | st::PSEUDO_ELEMENT | st::PSEUDO_ELEMENT_FUNCTION => {
                    sum[2] = sum[2].saturating_add(1);
                }
                st::PSEUDO_CLASS_FUNCTION => match (n.pseudo_type, n.data) {
                    (cf::WHERE, _) => {}
                    (cf::IS | cf::NOT | cf::HAS, data) => {
                        let first = match data {
                            PseudoData::List(l) => Some(l),
                            _ => None,
                        };
                        add(&mut sum, most_specific(ast, first));
                    }
                    (cf::NTH_CHILD | cf::NTH_LAST_CHILD, PseudoData::AnbOf(anb)) => {
                        sum[1] = sum[1].saturating_add(1);
                        add(&mut sum, most_specific(ast, anb.of));
                    }
                    _ => sum[1] = sum[1].saturating_add(1),
                },
                _ => {}
            }
            at = n.next;
        }
        ast.lists[id].specificity = pack(sum);
    }
}
