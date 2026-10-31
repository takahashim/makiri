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
use crate::html::dom::{NodeId, NodeType, ShortcutRule};
use crate::html::res::names as res;
use crate::ptr_table::{mix64, PtrMap, TableKey};
use falloc::VecGrow;

/// A `<select>` as a table key: its node word, which is never 0.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectKey(u64);

impl TableKey for SelectKey {
    const EMPTY: SelectKey = SelectKey(0);
    #[inline]
    fn table_hash(self) -> u64 {
        mix64(self.0)
    }
}

/// What the selectedness setting algorithm last left a select with, so the
/// next run can be decided from it and the option inserted, rather than by
/// walking every option again - which made a select of `n` options cost
/// `n^2` to build.
#[derive(Clone, Copy, Default)]
struct SelectState {
    /// Whether the fields below are the select's now. A full run makes them
    /// so; anything that could change its list of options otherwise clears it.
    valid: bool,
    /// The select's last child when the state was made: what the next option
    /// appended after it is checked against.
    tail: Option<NodeId>,
    /// The one option of its list that is selected, if one is: after a run
    /// of the algorithm there is never more than one.
    selected: Option<NodeId>,
    /// The first option of its list that is not disabled.
    first_enabled: Option<NodeId>,
    /// The first `<selectedcontent>` below it, found while `content_at` was
    /// still [`SelectStates::content_version`].
    content: Option<NodeId>,
    content_at: Option<u64>,
}

/// The selects' [`SelectState`]s of one parse.
#[derive(Default)]
pub(crate) struct SelectStates {
    index: PtrMap<SelectKey, u32>,
    states: Vec<SelectState>,
    /// A select was found inside another: the outer one's algorithm then
    /// sets the inner one's options, so no state is trusted for the parse.
    nested: bool,
    /// Moves on with every change that could put a different
    /// `<selectedcontent>` first below a select: one inserted, and the tree
    /// changes no insertion steps see (the adoption agency, foster
    /// parenting).
    content_version: u64,
}

impl SelectStates {
    fn get(&self, select: NodeId) -> Option<&SelectState> {
        let i = self.index.get(SelectKey(select.to_word() as u64))?;
        self.states.get(i as usize)
    }

    /// The select's state, made empty if it has none; `None` when there is
    /// no room for one, which leaves every answer to the full walk.
    fn entry(&mut self, select: NodeId) -> Option<&mut SelectState> {
        let key = SelectKey(select.to_word() as u64);
        let i = match self.index.get(key) {
            Some(i) => i as usize,
            None => {
                let i = self.states.len();
                self.states.falloc_push(SelectState::default()).ok()?;
                self.index.insert(key, u32::try_from(i).ok()?).ok()?;
                i
            }
        };
        self.states.get_mut(i)
    }

    /// Forget the select's list of options.
    fn invalidate(&mut self, select: NodeId) {
        if self.index.get(SelectKey(select.to_word() as u64)).is_some() {
            if let Some(st) = self.entry(select) {
                st.valid = false;
            }
        }
    }
}

/// What the selectedness setting algorithm does to a list of options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selectedness {
    /// Nothing: one option is selected, or none is and none is enabled.
    Keep,
    /// None is selected: select this one, the first enabled.
    Select(NodeId),
    /// Several are: unselect all but this one, the last.
    KeepOnly(NodeId),
}

/// Whether an HTML element `tag`, a child of the select, is an option or one
/// `list_of_options` walks into - a child the state cannot account for
/// without the walk. Not an `<optgroup>`: one directly in the select has no
/// non-`optgroup` element between it and the select, so the walk passes it.
fn holds_options(tag: usize) -> bool {
    matches!(
        tag,
        res::TAG_OPTION | res::TAG_SELECT | res::TAG_DATALIST | res::TAG_HR
    )
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
            self.node_state
                .selected
                .insert(el)
                .map_err(|()| TreeStatus::Oom)?;
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
                let select = self.nearest_ancestor_select(n);
                /* Every other select above it may list it too. */
                self.invalidate_selects_above(n, select);
                if let Some(select) = select {
                    self.selectedness_setting_algorithm(select, n);
                }
            } else if self.node_is(n, res::TAG_SELECTEDCONTENT) {
                self.node_state.selects.content_version += 1;
            } else if self.node_is(n, res::TAG_SELECT) && self.has_select_above(n) {
                self.node_state.selects.nested = true;
            }
            node = self.doc.preorder_next(root, n);
        }
    }

    /// Forget the list of options of every select above `n` but `except`.
    fn invalidate_selects_above(&mut self, n: NodeId, except: Option<NodeId>) {
        let mut up = self.doc.parent(n);
        while let Some(a) = up {
            if self.node_is(a, res::TAG_SELECT) && Some(a) != except {
                self.node_state.selects.invalidate(a);
            }
            up = self.doc.parent(a);
        }
    }

    fn has_select_above(&self, n: NodeId) -> bool {
        let mut up = self.doc.parent(n);
        while let Some(a) = up {
            if self.node_is(a, res::TAG_SELECT) {
                return true;
            }
            up = self.doc.parent(a);
        }
        false
    }

    /// The tree is about to change where no insertion steps see it: under
    /// `at` (foster parenting's insert, before `at`), or anywhere (the
    /// adoption agency, `at` `None`). Any select above `at` forgets its
    /// options, and every select its first `<selectedcontent>`.
    pub(crate) fn tree_moved(&mut self, at: Option<NodeId>) {
        self.node_state.selects.content_version += 1;
        if let Some(at) = at {
            self.invalidate_selects_above(at, None);
            if self.node_is(at, res::TAG_SELECT) {
                self.node_state.selects.invalidate(at);
            }
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

    /// `lxb_html_option_element_nearest_ancestor_select`: up from `option`
    /// through at most one `<optgroup>`, stopped by a `<datalist>`, `<hr>`
    /// or `<option>`.
    fn nearest_ancestor_select(&self, option: NodeId) -> Option<NodeId> {
        let mut optgroup = false;
        let mut node = self.doc.parent(option);
        while let Some(n) = node {
            if self.ns(n) == res::NS_HTML {
                match self.tag(n) {
                    res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => return None,
                    res::TAG_OPTGROUP if optgroup => return None,
                    res::TAG_OPTGROUP => optgroup = true,
                    res::TAG_SELECT => return Some(n),
                    _ => {}
                }
            }
            node = self.doc.parent(n);
        }
        None
    }

    /// `lxb_dom_element_attr_by_id`: the first attribute with local name
    /// `id`, in any namespace.
    fn attr_by_id(&self, el: NodeId, id: usize) -> Option<NodeId> {
        self.doc.attrs_of(el).find(|&x| self.tag(x) == id)
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
            if !self.node_state.valueless.contains(size) {
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

    /// Whether `option`, below `select`, is in its list of options: every
    /// element between them one `list_of_options` walks into.
    fn is_listed(&self, select: NodeId, option: NodeId) -> bool {
        let mut up = self.doc.parent(option);
        while let Some(a) = up {
            if a == select {
                return true;
            }
            if !self.walks_into(a, select) {
                return false;
            }
            up = self.doc.parent(a);
        }
        false
    }

    /// Whether `list_of_options` for `root` walks into `n`'s children.
    fn walks_into(&self, n: NodeId, root: NodeId) -> bool {
        if self.ns(n) != res::NS_HTML {
            return false;
        }
        match self.tag(n) {
            res::TAG_SELECT | res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => true,
            res::TAG_OPTGROUP => {
                let mut up = self.doc.parent(n);
                while let Some(o) = up {
                    if o == root {
                        return false;
                    }
                    if self.tag(o) != res::TAG_OPTGROUP && self.ns(o) == res::NS_HTML {
                        return true;
                    }
                    up = self.doc.parent(o);
                }
                false
            }
            _ => false,
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
    fn list_of_options(&self, select: NodeId) -> Result<(Vec<NodeId>, bool), TreeStatus> {
        let mut nested = false;
        let mut out = Vec::new();
        let root = select;
        let mut node = self.doc.first_child(root);
        while let Some(n) = node {
            if self.node_is(n, res::TAG_OPTION) {
                out.falloc_push(n).map_err(|()| TreeStatus::Oom)?;
            }
            if self.node_is(n, res::TAG_SELECT) {
                /* Its options are this select's too: see `nested`. */
                nested = true;
            }
            let descend = self.walks_into(n, root);
            node = if descend {
                self.doc.preorder_next(root, n)
            } else {
                self.doc.preorder_next_past(root, n)
            };
        }
        Ok((out, nested))
    }

    /// `lxb_html_select_selectedness_setting_algorithm`, run for the option
    /// `inserted`. When `inserted` was appended as the select's last option
    /// since its state was made, it is decided from that state
    /// ([`fast_selectedness`](Self::fast_selectedness)); otherwise by walking
    /// every option, which makes the state again.
    fn selectedness_setting_algorithm(&mut self, select: NodeId, inserted: NodeId) {
        if self.attr_by_id(select, res::ATTR_MULTIPLE).is_some() {
            return;
        }
        /* Step 1 */
        if self.display_size(select) != 1 {
            /* Lexbor reads step 1's counts uninitialised here; see the
             * module doc. */
            return;
        }
        if self.fast_selectedness(select, inserted) {
            return;
        }
        /* Lexbor answers a failed walk with an exception its callers
         * drop. */
        let Ok((options, nested)) = self.list_of_options(select) else {
            return;
        };
        if nested {
            self.node_state.selects.nested = true;
        }
        let (decision, first) = self.decide(&options);
        match decision {
            Selectedness::Keep => {}
            Selectedness::Select(f) => {
                let _ = self.node_state.selected.insert(f);
            }
            /* Step 2 */
            Selectedness::KeepOnly(last) => {
                for &o in &options {
                    if o != last && self.node_state.selected.contains(o) {
                        self.node_state.selected.remove(o);
                    }
                }
            }
        }
        let selected = options
            .iter()
            .copied()
            .find(|&o| self.node_state.selected.contains(o));
        let tail = self.doc.last_child(select);
        if let Some(st) = self.node_state.selects.entry(select) {
            *st = SelectState {
                valid: true,
                tail,
                selected,
                first_enabled: first,
                ..*st
            };
        }
    }

    /// What the algorithm does to `options`, and their first enabled one.
    fn decide(&self, options: &[NodeId]) -> (Selectedness, Option<NodeId>) {
        let mut first = None;
        let mut last = None;
        let mut selected = 0;
        for &o in options {
            if self.node_state.selected.contains(o) {
                selected += 1;
                last = Some(o);
            }
            if first.is_none() && !self.option_is_disabled(o) {
                first = Some(o);
            }
        }
        let decision = match (selected, first, last) {
            (0, Some(f), _) => Selectedness::Select(f),
            (2.., _, Some(l)) => Selectedness::KeepOnly(l),
            _ => Selectedness::Keep,
        };
        (decision, first)
    }

    /// The algorithm decided from the select's state, when the state is
    /// known to hold for every option but `inserted`: the select's children
    /// added since the state's tail are ones that could list no option
    /// (`holds_options`; an `<optgroup>` directly in the select is walked
    /// past), so the list is the state's - with `inserted` at its end when
    /// `inserted` is listed, which is checked as the select's last child with
    /// no children of its own. `false` when that does not hold, for the full
    /// walk.
    fn fast_selectedness(&mut self, select: NodeId, inserted: NodeId) -> bool {
        let states = &self.node_state.selects;
        if states.nested {
            return false;
        }
        let Some(st) = states.get(select).copied().filter(|st| st.valid) else {
            return false;
        };
        let d = &*self.doc;
        let listed = self.is_listed(select, inserted);
        if listed
            && (d.parent(inserted) != Some(select)
                || d.last_child(select) != Some(inserted)
                || d.first_child(inserted).is_some())
        {
            return false;
        }
        let mut between = d.last_child(select);
        if listed {
            between = d.prev(inserted);
        }
        while between != st.tail {
            match between {
                Some(n) if !(self.ns(n) == res::NS_HTML && holds_options(self.tag(n))) => {
                    between = d.prev(n);
                }
                /* The tail is gone, or an element between could list one. */
                _ => return false,
            }
        }
        if !listed {
            /* The list is the one the last run left: run again, the
             * algorithm finds nothing to change. */
            #[cfg(debug_assertions)]
            if let Ok((options, _)) = self.list_of_options(select) {
                assert_eq!(
                    (Selectedness::Keep, st.first_enabled),
                    self.decide(&options),
                    "the selectedness from the state is not the walk's"
                );
            }
            let tail = d.last_child(select);
            if let Some(entry) = self.node_state.selects.entry(select) {
                entry.tail = tail;
            }
            return true;
        }
        let is_selected = self.node_state.selected.contains(inserted);
        let first = st
            .first_enabled
            .or((!self.option_is_disabled(inserted)).then_some(inserted));
        let decision = match (st.selected.is_some(), is_selected, first) {
            (false, false, Some(f)) => Selectedness::Select(f),
            (true, true, _) => Selectedness::KeepOnly(inserted),
            _ => Selectedness::Keep,
        };
        #[cfg(debug_assertions)]
        if let Ok((options, _)) = self.list_of_options(select) {
            assert_eq!(
                (decision, first),
                self.decide(&options),
                "the selectedness from the state is not the walk's"
            );
        }
        let selected = match decision {
            Selectedness::Select(f) => {
                let _ = self.node_state.selected.insert(f);
                Some(f)
            }
            Selectedness::KeepOnly(last) => {
                if let Some(old) = st.selected {
                    self.node_state.selected.remove(old);
                }
                Some(last)
            }
            Selectedness::Keep => {
                if is_selected {
                    Some(inserted)
                } else {
                    st.selected
                }
            }
        };
        if let Some(entry) = self.node_state.selects.entry(select) {
            *entry = SelectState {
                tail: Some(inserted),
                selected,
                first_enabled: first,
                ..st
            };
        }
        true
    }

    /// `lxb_html_select_get_enabled_selectedcontent`: the first
    /// `<selectedcontent>` below the select, unless the select is
    /// `multiple`. (A `<selectedcontent>` is never disabled: Lexbor's
    /// insertion steps that would disable one are not wired to it.)
    /// The walk is remembered per select until the
    /// first `<selectedcontent>` below one could have changed
    /// (`SelectStates::content_version`): a selected option popped looks it
    /// up again, and walking each time made a select's options cost `n^2`.
    fn enabled_selectedcontent(&mut self, select: NodeId) -> Option<NodeId> {
        if self.attr_by_id(select, res::ATTR_MULTIPLE).is_some() {
            return None;
        }
        let version = self.node_state.selects.content_version;
        if let Some(st) = self.node_state.selects.get(select) {
            if st.content_at == Some(version) {
                #[cfg(debug_assertions)]
                assert_eq!(st.content, self.first_selectedcontent(select));
                return st.content;
            }
        }
        let found = self.first_selectedcontent(select);
        if let Some(st) = self.node_state.selects.entry(select) {
            st.content = found;
            st.content_at = Some(version);
        }
        found
    }

    /// `lxb_dom_node_simple_walk` below `select`: its first
    /// `<selectedcontent>`.
    fn first_selectedcontent(&self, select: NodeId) -> Option<NodeId> {
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
        if !self.node_state.selected.contains(option) {
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
