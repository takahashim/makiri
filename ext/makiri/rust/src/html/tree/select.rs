//! What Lexbor's DOM does on its own while the tree builder works: the
//! option element's selectedness and the `<selectedcontent>` it is cloned
//! into (`html/interfaces/option_element.c`, `select_element.c`,
//! `selectedcontent_element.c`, reached through `html/element_steps.c` and
//! `html/attribute_steps.c`).
//!
//! Lexbor runs these from its DOM's mutation hooks: an attribute appended to
//! an `option` (`selected` sets its selectedness), a node inserted as a child
//! (an HTML `option` in it runs its select's selectedness setting
//! algorithm), and an `option` popped off the stack of open elements (a
//! selected one is cloned into its select's `<selectedcontent>`). The tree
//! builder calls the same steps at the same points; the arena itself has no
//! hooks. Only the clone is visible in the tree, but when it happens depends
//! on all of it.
//!
//! Two departures, both where Lexbor's behaviour is not defined:
//!
//! - The selectedness setting algorithm reads its step-1 counts in step 2
//!   even when step 1 did not run (a display size other than 1), which is an
//!   uninitialised read in Lexbor. Here step 2 does not run then.
//! - Lexbor writes an `option`'s selectedness through any element whose tag
//!   is `option`, whatever its namespace and so whatever its struct. Here
//!   only the set of selected nodes changes, which nothing outside the HTML
//!   namespace reads.

use super::{TreeBuilder, TreeStatus};
use crate::falloc::VecPush;
use crate::html::dom::{NodeId, NodeType, ShortcutRule};
use crate::html::res::names as res;

/// Where [`TreeBuilder::walk_to_select`] ended.
pub(crate) enum SelectWalk {
    /// At this `<select>`.
    Select(NodeId),
    /// At an element that keeps the option from any select.
    Blocked,
    /// Past the top of the tree, through an `<optgroup>` or not.
    OffTop { optgroup: bool },
}

impl TreeBuilder<'_> {
    /// `lxb_dom_element_attr_append` with Lexbor's attribute steps.
    pub(crate) fn append_attr_with_steps(
        &mut self,
        el: NodeId,
        attr: NodeId,
    ) -> Result<(), TreeStatus> {
        self.doc.link_attribute(el, attr, ShortcutRule::Lexbor);
        /* `lxb_html_option_attr_steps_change`, by tag id alone. */
        if self.tag(el) == res::TAG_OPTION && self.tag(attr) == res::ATTR_SELECTED {
            self.selected.insert(el).map_err(|()| TreeStatus::Oom)?;
        }
        Ok(())
    }

    /// `lxb_dom_node_insert_child` with its insertion steps: the steps run
    /// for `node` and each of its descendants, in tree order.
    pub(crate) fn append_child_with_steps(&mut self, parent: NodeId, node: NodeId) {
        self.doc.append_child(parent, node);
        self.insertion_steps(node);
    }

    /// `lxb_html_element_steps_insertion` over `root` and its descendants
    /// (`lxb_dom_node_shadow_including_descendants`).
    fn insertion_steps(&mut self, root: NodeId) {
        let mut node = Some(root);
        while let Some(n) = node {
            if self.node_is(n, res::TAG_OPTION) {
                /* `lxb_html_option_element_insert_steps` */
                if let Some(select) = self.nearest_ancestor_select(n) {
                    self.selectedness_setting_algorithm(select);
                }
            }
            node = self.doc.preorder_next(root, n);
        }
    }

    /// The open-elements pop callback (`lxb_html_tree_open_elements_pop_cb`):
    /// an HTML `option` leaving the stack may be cloned into its select's
    /// `<selectedcontent>`.
    pub(crate) fn pop_steps(&mut self, n: NodeId) {
        if self.node_is(n, res::TAG_OPTION) {
            /* `lxb_html_option_element_pop_open_elements`; Lexbor's tree
             * builder ignores what it returns everywhere but in a status it
             * then overwrites. */
            let _ = self.maybe_clone_to_selectedcontent(n);
        }
    }

    /// `lxb_html_option_element_nearest_ancestor_select`.
    fn nearest_ancestor_select(&self, option: NodeId) -> Option<NodeId> {
        match self.walk_to_select(option) {
            SelectWalk::Select(n) => Some(n),
            SelectWalk::Blocked | SelectWalk::OffTop { .. } => None,
        }
    }

    /// The walk up from `option` to the select it belongs to, which Lexbor's
    /// option steps and the guard's option count both take: through at
    /// most one `<optgroup>`, stopped by a `<datalist>`, `<hr>` or `<option>`.
    pub(crate) fn walk_to_select(&self, option: NodeId) -> SelectWalk {
        let mut optgroup = false;
        let mut node = self.doc.parent(option);
        while let Some(n) = node {
            if self.ns(n) == res::NS_HTML {
                match self.tag(n) {
                    res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => {
                        return SelectWalk::Blocked
                    }
                    res::TAG_OPTGROUP if optgroup => return SelectWalk::Blocked,
                    res::TAG_OPTGROUP => optgroup = true,
                    res::TAG_SELECT => return SelectWalk::Select(n),
                    _ => {}
                }
            }
            node = self.doc.parent(n);
        }
        SelectWalk::OffTop { optgroup }
    }

    /// `lxb_dom_element_attr_by_id`: the first attribute with local name
    /// `id`, in any namespace.
    fn attr_by_id(&self, el: NodeId, id: usize) -> Option<NodeId> {
        let mut a = self.doc.first_attr(el);
        while let Some(x) = a {
            if self.tag(x) == id {
                return Some(x);
            }
            a = self.doc.next_attr(x);
        }
        None
    }

    /// `lxb_html_option_is_disabled`.
    fn option_is_disabled(&self, option: NodeId) -> bool {
        if self.attr_by_id(option, res::ATTR_DISABLED).is_some() {
            return true;
        }
        let mut node = self.doc.parent(option);
        while let Some(n) = node {
            if self.ns(n) == res::NS_HTML {
                match self.tag(n) {
                    res::TAG_SELECT | res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => {
                        return false
                    }
                    res::TAG_OPTGROUP => {
                        return self.attr_by_id(n, res::ATTR_DISABLED).is_some();
                    }
                    _ => {}
                }
            }
            node = self.doc.parent(n);
        }
        false
    }

    /// `lxb_html_select_selectedness_display_size`.
    fn display_size(&self, select: NodeId) -> u64 {
        if let Some(size) = self.attr_by_id(select, res::ATTR_SIZE) {
            if !self.valueless.contains(size) {
                if let Some(v) = parse_nonneg_integer(self.doc.attr_value(size).unwrap_or_default())
                {
                    return v;
                }
            }
        }
        if self.attr_by_id(select, res::ATTR_MULTIPLE).is_none() {
            1
        } else {
            4
        }
    }

    /// `lxb_html_select_list_of_options`, as Lexbor walks it: into an
    /// element's children only when the element is a `select`, `datalist`,
    /// `hr` or `option`, or an `optgroup` with a non-`optgroup` HTML element
    /// between it and the select. That is NOT the Standard's "get the list
    /// of options", which enters every other element (an `optgroup`'s and a
    /// `div`'s options count); taking it here as it stands made the walk,
    /// run once per inserted option, cost options x nodes under the select,
    /// so it waits for a walk that is not repeated per option.
    fn list_of_options(&self, select: NodeId) -> Result<Vec<NodeId>, TreeStatus> {
        let mut out = Vec::new();
        let root = select;
        let mut node = self.doc.first_child(root);
        while let Some(n) = node {
            if self.node_is(n, res::TAG_OPTION) {
                out.falloc_push(n).map_err(|()| TreeStatus::Oom)?;
            }
            let mut descend = false;
            if self.ns(n) == res::NS_HTML {
                match self.tag(n) {
                    res::TAG_SELECT | res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => {
                        descend = true
                    }
                    res::TAG_OPTGROUP => {
                        let mut up = self.doc.parent(n);
                        while let Some(o) = up {
                            if o == root {
                                break;
                            }
                            if self.tag(o) != res::TAG_OPTGROUP && self.ns(o) == res::NS_HTML {
                                descend = true;
                                break;
                            }
                            up = self.doc.parent(o);
                        }
                    }
                    _ => {}
                }
            }
            node = if descend {
                self.doc.preorder_next(root, n)
            } else {
                self.doc.preorder_next_past(root, n)
            };
        }
        Ok(out)
    }

    /// `lxb_html_select_selectedness_setting_algorithm`.
    fn selectedness_setting_algorithm(&mut self, select: NodeId) {
        if self.attr_by_id(select, res::ATTR_MULTIPLE).is_some() {
            return;
        }
        /* Step 1 */
        if self.display_size(select) != 1 {
            /* Lexbor reads step 1's counts uninitialised here; see the
             * module doc. */
            return;
        }
        /* Lexbor answers a failed walk with an exception its callers
         * drop. */
        let Ok(options) = self.list_of_options(select) else {
            return;
        };
        let mut first = None;
        let mut last = None;
        let mut selected = 0;
        for &o in &options {
            if self.selected.contains(o) {
                selected += 1;
                last = Some(o);
            }
            if first.is_none() && !self.option_is_disabled(o) {
                first = Some(o);
            }
        }
        if selected == 0 {
            if let Some(f) = first {
                let _ = self.selected.insert(f);
            }
            return;
        }
        /* Step 2 */
        if selected >= 2 {
            for &o in &options {
                if self.selected.contains(o) && Some(o) != last {
                    self.selected.remove(o);
                }
            }
        }
    }

    /// `lxb_html_select_get_enabled_selectedcontent`: the first
    /// `<selectedcontent>` below the select, unless the select is
    /// `multiple`. (A `<selectedcontent>` is never disabled: Lexbor's
    /// insertion steps that would disable one are not wired to it.)
    fn enabled_selectedcontent(&self, select: NodeId) -> Option<NodeId> {
        if self.attr_by_id(select, res::ATTR_MULTIPLE).is_some() {
            return None;
        }
        /* `lxb_dom_node_simple_walk`, below the select. */
        let root = select;
        let mut node = self.doc.first_child(root);
        while let Some(n) = node {
            if self.node_is(n, res::TAG_SELECTEDCONTENT) {
                return Some(n);
            }
            node = self.doc.preorder_next(root, n);
        }
        None
    }

    /// `lxb_html_option_maybe_clone_to_selectedcontent`.
    fn maybe_clone_to_selectedcontent(&mut self, option: NodeId) -> Result<(), TreeStatus> {
        if !self.selected.contains(option) {
            return Ok(());
        }
        let Some(select) = self.nearest_ancestor_select(option) else {
            return Ok(());
        };
        let Some(sc) = self.enabled_selectedcontent(select) else {
            return Ok(());
        };
        /* `lxb_html_selectedcontent_clone_option`: clone the option's
         * children, then replace all of the selectedcontent's children with
         * them. */
        let mut clones = Vec::new();
        let mut child = self.doc.first_child(option);
        while let Some(c) = child {
            let made = self.clone_deep(c)?;
            clones.falloc_push(made).map_err(|()| TreeStatus::Oom)?;
            child = self.doc.next(c);
        }
        while let Some(old) = self.doc.first_child(sc) {
            self.doc.detach(old);
        }
        for made in clones {
            self.append_child_with_steps(sc, made);
        }
        Ok(())
    }

    /// `lxb_dom_node_clone(node, true)`: Lexbor's `importNode` into the same
    /// document - every node's kind, name fields, data and source offset,
    /// an element's written name and attributes (appended with their steps),
    /// and the children, each inserted with its steps. A template's
    /// contents are not copied, as Lexbor's import does not copy them.
    fn clone_deep(&mut self, src: NodeId) -> Result<NodeId, TreeStatus> {
        let root = self.clone_one(src)?;
        /* (source, its clone) of each node whose children are still to copy */
        let mut todo = Vec::new();
        todo.falloc_push((src, root))
            .map_err(|()| TreeStatus::Oom)?;
        while let Some((s, d)) = todo.pop() {
            let mut c = self.doc.first_child(s);
            while let Some(sc) = c {
                let made = self.clone_one(sc)?;
                self.append_child_with_steps(d, made);
                todo.falloc_push((sc, made)).map_err(|()| TreeStatus::Oom)?;
                c = self.doc.next(sc);
            }
        }
        Ok(root)
    }

    /// One node of [`clone_deep`](Self::clone_deep), detached: anything
    /// but an element as the arena copies it, an element with its attributes
    /// made and appended by Lexbor's steps.
    fn clone_one(&mut self, src: NodeId) -> Result<NodeId, TreeStatus> {
        if self.doc.kind(src) != NodeType::Element {
            return self.doc.copy_within(src, false).map_err(TreeStatus::from);
        }
        let d = &mut *self.doc;
        let el = d.create_element(d.local(src) as usize, d.ns(src), d.prefix(src), None)?;
        let written = d.written_id(src);
        d.set_written_id(el, written);
        if let Some(o) = d.source_offset(src) {
            d.set_source_offset(el, o);
        }
        let mut a = self.doc.first_attr(src);
        while let Some(x) = a {
            let attr = self.copy_attr(x, self.doc.prefix(x), None)?;
            self.append_attr_with_steps(el, attr)?;
            a = self.doc.next_attr(x);
        }
        Ok(el)
    }
}

/// `lxb_html_common_parsing_nonneg_integer`, with Lexbor's wrapping
/// arithmetic.
fn parse_nonneg_integer(data: &[u8]) -> Option<u64> {
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | 0x0C | b'\r');
    let mut i = 0;
    while i < data.len() && ws(data[i]) {
        i += 1;
    }
    let mut positive = true;
    match data.get(i) {
        None => return None,
        Some(b'-') => {
            positive = false;
            i += 1;
        }
        Some(b'+') => i += 1,
        Some(_) => {}
    }
    if !data.get(i).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u64 = 0;
    while let Some(&c) = data.get(i).filter(|c| c.is_ascii_digit()) {
        value = value.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
        i += 1;
    }
    let signed = if positive {
        value as i64
    } else {
        (value as i64).wrapping_neg()
    };
    (signed >= 0).then_some(signed as u64)
}
