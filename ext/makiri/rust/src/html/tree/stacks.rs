//! The stack of open elements (`html/tree/open_elements.c`) and the list of
//! active formatting elements (`html/tree/active_formatting.c`).
//!
//! Both are types of this module whose vector only this module reaches:
//! the rest of the tree builder reads them as slices, and changes them
//! through these methods - which is what keeps every pop running Lexbor's
//! pop callback ([`pop_steps`]: an option leaving the stack may be cloned
//! into its select's `<selectedcontent>`). The adoption agency
//! (`actions.rs`) also replaces entries in place, as Lexbor's does
//! ([`OpenElements::replace`], [`ActiveFormatting::replace`]).
//!
//! [`pop_steps`]: TreeBuilder::pop_steps

use super::{Tok, TreeBuilder, TreeStatus};
use crate::compat::{self, Fix};
use crate::falloc::{Reserve, VecPush};
use crate::html::dom::NodeId;
use crate::html::res::names as res;

/// The stack of open elements, bottom first. Read as a slice; pushed and
/// popped only through `TreeBuilder`'s methods below.
#[derive(Default)]
pub(crate) struct OpenElements(Vec<NodeId>);

impl core::ops::Deref for OpenElements {
    type Target = [NodeId];
    #[inline]
    fn deref(&self) -> &[NodeId] {
        &self.0
    }
}

impl OpenElements {
    /// Put `n` in entry `idx`'s place (the adoption agency's step 14.7),
    /// which runs no pop steps: the element replaced stays in the tree.
    #[inline]
    pub(crate) fn replace(&mut self, idx: usize, n: NodeId) {
        self.0[idx] = n;
    }
}

/// The list of active formatting elements; `None` is a marker. Read as a
/// slice; changed through `TreeBuilder`'s methods below.
#[derive(Default)]
pub(crate) struct ActiveFormatting(Vec<Option<NodeId>>);

impl core::ops::Deref for ActiveFormatting {
    type Target = [Option<NodeId>];
    #[inline]
    fn deref(&self) -> &[Option<NodeId>] {
        &self.0
    }
}

impl ActiveFormatting {
    /// Put `n` in entry `idx`'s place (the adoption agency's step 14.7).
    #[inline]
    pub(crate) fn replace(&mut self, idx: usize, n: NodeId) {
        self.0[idx] = Some(n);
    }
}

impl TreeBuilder<'_> {
    /* ---- open elements ---- */

    /// `lxb_html_tree_open_elements_push`.
    #[inline]
    pub(crate) fn open_push(&mut self, n: NodeId) -> Result<(), TreeStatus> {
        self.open.0.falloc_push(n).map_err(|()| TreeStatus::Oom)
    }

    /// `lxb_html_tree_open_elements_pop`.
    #[inline]
    pub(crate) fn open_pop(&mut self) -> Option<NodeId> {
        let n = self.open.0.pop()?;
        self.pop_steps(n);
        Some(n)
    }

    /// `lxb_html_tree_open_elements_first`.
    #[inline]
    pub(crate) fn open_first(&self) -> Option<NodeId> {
        self.open.0.first().copied()
    }

    /// The number of open elements.
    #[inline]
    pub(crate) fn open_len(&self) -> usize {
        self.open.0.len()
    }

    /// `lxb_html_tree_open_elements_get`.
    #[inline]
    pub(crate) fn open_get(&self, idx: usize) -> Option<NodeId> {
        self.open.0.get(idx).copied()
    }

    /// `lxb_html_tree_open_elements_insert_after`.
    pub(crate) fn open_insert_after(&mut self, n: NodeId, idx: usize) -> Result<(), TreeStatus> {
        array_insert(&mut self.open.0, idx + 1, n)
    }

    /// `lxb_html_tree_open_elements_remove_by_node`.
    pub(crate) fn open_remove_by_node(&mut self, n: NodeId) {
        if let Some(i) = self.open.0.iter().rposition(|&x| x == n) {
            self.open.0.remove(i);
        }
    }

    /// `lxb_html_tree_process_abort`'s `lexbor_array_clean`: every element
    /// off the stack at once, with no pop steps.
    pub(crate) fn open_abandon(&mut self) {
        self.open.0.clear();
    }

    /// The Standard's "clear the stack back to a ... context": pop until the
    /// current node is an HTML element with one of the tags `stop`.
    pub(crate) fn clear_stack_back_to(&mut self, stop: &[usize]) {
        while let Some(c) = self.current_node() {
            if self.ns(c) == res::NS_HTML && stop.contains(&self.tag(c)) {
                return;
            }
            self.open_pop();
        }
    }

    /// `lxb_html_tree_open_elements_pop_until_tag_id`.
    pub(crate) fn open_pop_until_tag_id(&mut self, tag: usize, ns: usize, exclude: bool) {
        while let Some(n) = self.open.0.pop() {
            if self.tag(n) == tag && self.ns(n) == ns {
                if exclude {
                    self.pop_steps(n);
                } else {
                    self.open.0.push(n);
                }
                break;
            }
            self.pop_steps(n);
        }
    }

    /// `lxb_html_tree_open_elements_pop_until_h123456`.
    pub(crate) fn open_pop_until_h123456(&mut self) {
        while let Some(n) = self.open.0.pop() {
            self.pop_steps(n);
            if is_h123456(self.tag(n)) && self.ns(n) == res::NS_HTML {
                return;
            }
        }
    }

    /// `lxb_html_tree_open_elements_pop_until_td_th`.
    pub(crate) fn open_pop_until_td_th(&mut self) {
        while let Some(n) = self.open.0.pop() {
            self.pop_steps(n);
            if matches!(self.tag(n), res::TAG_TD | res::TAG_TH) && self.ns(n) == res::NS_HTML {
                return;
            }
        }
    }

    /// `lxb_html_tree_open_elements_pop_until_node`.
    pub(crate) fn open_pop_until_node(&mut self, node: NodeId, exclude: bool) {
        while let Some(n) = self.open.0.pop() {
            if n == node {
                if exclude {
                    self.pop_steps(n);
                } else {
                    self.open.0.push(n);
                }
                break;
            }
            self.pop_steps(n);
        }
    }

    /// `lxb_html_tree_open_elements_pop_all`: every element off the stack,
    /// each with its pop steps.
    pub(crate) fn open_pop_all(&mut self) {
        while let Some(n) = self.open.0.pop() {
            self.pop_steps(n);
        }
    }

    /// `lxb_html_tree_open_elements_find_by_node`.
    pub(crate) fn open_find_by_node(&self, n: NodeId) -> Option<usize> {
        self.open.0.iter().position(|&x| x == n)
    }

    /// `lxb_html_tree_open_elements_find_by_node_reverse`.
    pub(crate) fn open_find_by_node_reverse(&self, n: NodeId) -> Option<usize> {
        self.open.0.iter().rposition(|&x| x == n)
    }

    /// `lxb_html_tree_open_elements_find`.
    pub(crate) fn open_find(&self, tag: usize, ns: usize) -> Option<(NodeId, usize)> {
        self.open
            .iter()
            .position(|&x| self.tag(x) == tag && self.ns(x) == ns)
            .map(|i| (self.open.0[i], i))
    }

    /// `lxb_html_tree_open_elements_find_reverse`.
    pub(crate) fn open_find_reverse(&self, tag: usize, ns: usize) -> Option<(NodeId, usize)> {
        self.open
            .iter()
            .rposition(|&x| self.tag(x) == tag && self.ns(x) == ns)
            .map(|i| (self.open.0[i], i))
    }

    /// `lxb_html_tree_template_insertion_push`.
    pub(crate) fn template_modes_push(&mut self, mode: super::Mode) -> Result<(), TreeStatus> {
        self.template_modes
            .falloc_push(mode)
            .map_err(|()| TreeStatus::Oom)
    }

    /// Pop the current template insertion mode.
    #[inline]
    pub(crate) fn template_modes_pop(&mut self) -> Option<super::Mode> {
        self.template_modes.pop()
    }

    /// Whether the stack of template insertion modes is not empty.
    #[inline]
    pub(crate) fn in_template_mode(&self) -> bool {
        !self.template_modes.is_empty()
    }

    /* ---- active formatting elements ---- */

    /// `lxb_html_tree_active_formatting_push`.
    #[inline]
    pub(crate) fn afe_push(&mut self, n: Option<NodeId>) -> Result<(), TreeStatus> {
        self.afe.0.falloc_push(n).map_err(|()| TreeStatus::Oom)
    }

    /// `lxb_html_tree_active_formatting_push_marker`.
    #[inline]
    pub(crate) fn afe_push_marker(&mut self) -> Result<(), TreeStatus> {
        self.afe_push(None)
    }

    /// `lxb_html_tree_active_formatting_insert`.
    pub(crate) fn afe_insert(&mut self, n: NodeId, idx: usize) -> Result<(), TreeStatus> {
        array_insert(&mut self.afe.0, idx, Some(n))
    }

    /// `lxb_html_tree_active_formatting_remove` (`lexbor_array_delete` of
    /// one entry, which does nothing past the end).
    pub(crate) fn afe_remove(&mut self, idx: usize) {
        if idx < self.afe.0.len() {
            self.afe.0.remove(idx);
        }
    }

    /// `lxb_html_tree_active_formatting_up_to_last_marker`.
    pub(crate) fn afe_up_to_last_marker(&mut self) {
        while let Some(e) = self.afe.0.pop() {
            if e.is_none() {
                break;
            }
        }
    }

    /// `lxb_html_tree_active_formatting_remove_by_node`.
    pub(crate) fn afe_remove_by_node(&mut self, n: NodeId) {
        if let Some(i) = self.afe.0.iter().rposition(|&x| x == Some(n)) {
            self.afe.0.remove(i);
        }
    }

    /// `lxb_html_tree_active_formatting_find_by_node_reverse`.
    pub(crate) fn afe_find_by_node_reverse(&self, n: NodeId) -> Option<usize> {
        self.afe.0.iter().rposition(|&x| x == Some(n))
    }

    /// `lxb_html_tree_active_formatting_reconstruct_elements`.
    pub(crate) fn afe_reconstruct(
        &mut self,
        tkz: &crate::html::tokenizer::Tokenizer<'_, '_>,
    ) -> Result<(), TreeStatus> {
        /* Step 1 */
        let Some(&last) = self.afe.0.last() else {
            return Ok(());
        };
        /* Step 2-3 */
        let mut idx = self.afe.0.len() - 1;
        let open_or_marker = |t: &Self, e: Option<NodeId>| match e {
            None => true,
            Some(n) => t.open_find_by_node_reverse(n).is_some(),
        };
        if open_or_marker(self, last) {
            return Ok(());
        }
        /* Step 4-6: rewind */
        while idx != 0 {
            idx -= 1;
            if open_or_marker(self, self.afe.0[idx]) {
                /* Step 7 */
                idx += 1;
                break;
            }
        }
        /* Step 8-10: create */
        while idx < self.afe.0.len() {
            let Some(node) = self.afe.0[idx] else {
                /* A marker is never past the rewind point. */
                idx += 1;
                continue;
            };
            let mut fake = Tok::fake(self.tag(node));
            fake.base = Some(node);
            let el = self.insert_html_element(tkz, &fake)?;
            /* Step 9 */
            self.afe.0[idx] = Some(el);
            /* Step 10 */
            idx += 1;
        }
        Ok(())
    }

    /// `lxb_html_tree_active_formatting_between_last_marker`.
    pub(crate) fn afe_between_last_marker(&self, tag: usize) -> Option<(NodeId, usize)> {
        for idx in (0..self.afe.0.len()).rev() {
            let n = self.afe.0[idx]?;
            if self.tag(n) == tag && self.ns(n) == res::NS_HTML {
                return Some((n, idx));
            }
        }
        None
    }

    /// `lxb_html_tree_active_formatting_push_with_check_dupl`: the Noah's Ark
    /// clause.
    pub(crate) fn afe_push_with_check_dupl(&mut self, node: NodeId) -> Result<(), TreeStatus> {
        let mut idx = self.afe.0.len();
        let mut earliest = idx.saturating_sub(1);
        let mut count = 0;
        while idx != 0 {
            idx -= 1;
            let Some(e) = self.afe.0[idx] else { break };
            if self.tag(e) == self.tag(node)
                && self.ns(e) == self.ns(node)
                && self.element_compare(e, node)
            {
                count += 1;
                earliest = idx;
            }
        }
        if count >= 3 {
            self.afe_remove(earliest);
        }
        self.afe_push(Some(node))
    }

    /// `lxb_dom_element_compare`: the same name and the same attributes, in
    /// any order.
    fn element_compare(&self, first: NodeId, second: NodeId) -> bool {
        let d = &*self.doc;
        if d.local(first) != d.local(second)
            || d.ns(first) != d.ns(second)
            || d.written_id(first) != d.written_id(second)
        {
            return false;
        }
        let count =
            |n: NodeId| core::iter::successors(d.first_attr(n), |&a| d.next_attr(a)).count();
        if count(first) != count(second) {
            return false;
        }
        let mut f = d.first_attr(first);
        while let Some(fa) = f {
            let found = core::iter::successors(d.first_attr(second), |&a| d.next_attr(a))
                .any(|sa| self.attr_compare(fa, sa));
            if !found {
                return false;
            }
            f = d.next_attr(fa);
        }
        true
    }

    /// `lxb_dom_attr_compare`. The Standard compares values, and an attribute
    /// written with no value has the value "" - the same as `x=""`; Lexbor
    /// tells the two apart (P10).
    fn attr_compare(&self, first: NodeId, second: NodeId) -> bool {
        let d = &*self.doc;
        if d.local(first) != d.local(second)
            || d.ns(first) != d.ns(second)
            || d.written_id(first) != d.written_id(second)
        {
            return false;
        }
        if compat::fixed(Fix::P10) {
            let value = |a| {
                if self.valueless.contains(a) {
                    &[][..]
                } else {
                    d.attr_value(a).unwrap_or_default()
                }
            };
            return value(first) == value(second);
        }
        match (
            self.valueless.contains(first),
            self.valueless.contains(second),
        ) {
            (true, true) => true,
            (false, false) => d.attr_value(first) == d.attr_value(second),
            _ => false,
        }
    }
}

/// `lexbor_array_insert`: insert at `idx`. Past the end Lexbor grows the
/// array to `idx` with NULL entries; no caller inserts past the end (the
/// furthest is at the length), so here that appends.
fn array_insert<T: Copy>(v: &mut Vec<T>, idx: usize, value: T) -> Result<(), TreeStatus> {
    if idx >= v.len() {
        return v.falloc_push(value).map_err(|()| TreeStatus::Oom);
    }
    v.falloc_reserve(1).map_err(|()| TreeStatus::Oom)?;
    v.insert(idx, value);
    Ok(())
}

#[inline]
pub(crate) fn is_h123456(tag: usize) -> bool {
    matches!(
        tag,
        res::TAG_H1 | res::TAG_H2 | res::TAG_H3 | res::TAG_H4 | res::TAG_H5 | res::TAG_H6
    )
}
