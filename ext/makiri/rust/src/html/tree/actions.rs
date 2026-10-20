//! The tree builder's actions: `html/tree.c` past the dispatcher - the
//! appropriate place for inserting a node, creating and inserting elements,
//! character data, comments and processing instructions, the attribute
//! adjustments, implied end tags, resetting the insertion mode, the scope
//! tests, and the adoption agency algorithm.

use super::error::TreeErrorId as E;
use super::{AttrAdjust, Ipos, Mode, Tok, TreeBuilder, TreeStatus};
use crate::compat::{self, Fix};
use crate::html::dom::{AttrValue, NodeId, NodeName, NodeType};
use crate::html::res::names as res;
use crate::html::res::tree as rt;
use crate::html::tokenizer::TextContent;
use crate::html::tokenizer::{flags, Tokenizer};

/// An attribute about to be made: Lexbor's name fields, which the
/// adjustments rewrite before the attribute is appended.
#[derive(Clone, Copy)]
struct AttrSpec {
    local: usize,
    ns: usize,
    prefix: u32,
    written: u32,
}

/// `lxb_html_tag_is_category`.
#[inline]
pub(crate) fn is_category(tag: usize, ns: usize, cat: u8) -> bool {
    match rt::TAG_CATS.get(tag).and_then(|r| r.get(ns)) {
        Some(c) => c & cat != 0,
        None => rt::CAT_ORDINARY & cat != 0,
    }
}

/// `lxb_html_tag_is_void`.
#[inline]
pub(crate) fn is_void(tag: usize) -> bool {
    matches!(
        tag,
        res::TAG_AREA
            | res::TAG_BASE
            | res::TAG_BR
            | res::TAG_COL
            | res::TAG_EMBED
            | res::TAG_HR
            | res::TAG_IMG
            | res::TAG_INPUT
            | res::TAG_LINK
            | res::TAG_META
            | res::TAG_SOURCE
            | res::TAG_TRACK
            | res::TAG_WBR
    )
}

impl TreeBuilder<'_> {
    /// `lxb_html_tree_appropriate_place_inserting_node`.
    pub(crate) fn appropriate_place(
        &self,
        override_target: Option<NodeId>,
    ) -> Option<(NodeId, Ipos)> {
        let mut ipos = Ipos::Child;
        let target = match override_target {
            Some(t) => t,
            None => self.current_node()?,
        };
        let adjusted;
        if self.foster_parenting
            && self.ns(target) == res::NS_HTML
            && matches!(
                self.tag(target),
                res::TAG_TABLE | res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD | res::TAG_TR
            )
        {
            let last_temp = self.open_find_reverse(res::TAG_TEMPLATE, res::NS_HTML);
            let last_table = self.open_find_reverse(res::TAG_TABLE, res::NS_HTML);
            match (last_temp, last_table) {
                (Some((temp, ti)), table)
                    if table.is_none() || table.is_some_and(|(_, tbi)| ti > tbi) =>
                {
                    return Some((self.doc.template_content(temp)?, Ipos::Child));
                }
                (_, None) => {
                    adjusted = self.open_first()?;
                }
                (_, Some((table, _))) if self.doc.parent(table).is_some() => {
                    adjusted = table;
                    ipos = Ipos::Before;
                }
                (_, Some((_, tbi))) => {
                    adjusted = self.open_get(tbi.checked_sub(1)?)?;
                }
            }
        } else {
            adjusted = target;
        }
        /* "If the adjusted insertion location is inside a template element,
         * let it instead be inside the template element's template
         * contents" - Lexbor tests the location itself. */
        if self.node_is(adjusted, res::TAG_TEMPLATE) {
            return Some((self.doc.template_content(adjusted)?, ipos));
        }
        Some((adjusted, ipos))
    }

    /// `lxb_html_tree_insert_node`.
    #[inline]
    pub(crate) fn insert_node(&mut self, to: NodeId, node: NodeId, ipos: Ipos) {
        match ipos {
            Ipos::Before => self.doc.insert_before(to, node),
            Ipos::Child => self.append_child_with_steps(to, node),
        }
    }

    /// `lxb_html_tree_insert_foreign_element`.
    pub(crate) fn insert_foreign_element(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        tok: &Tok,
        ns: usize,
        adjust: Option<AttrAdjust>,
    ) -> Result<NodeId, TreeStatus> {
        let (pos, ipos) = self.appropriate_place(None).ok_or(TreeStatus::Error)?;
        let el = self.create_element_for_token(tkz, tok, ns, adjust)?;
        self.insert_node(pos, el, ipos);
        self.open_push(el)?;
        Ok(el)
    }

    /// `lxb_html_tree_insert_html_element`.
    #[inline]
    pub(crate) fn insert_html_element(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        tok: &Tok,
    ) -> Result<NodeId, TreeStatus> {
        self.insert_foreign_element(tkz, tok, res::NS_HTML, None)
    }

    /// `lxb_html_tree_create_node` for an element (`lxb_html_interface_create`).
    pub(crate) fn create_element_node(
        &mut self,
        tag: usize,
        ns: usize,
    ) -> Result<NodeId, TreeStatus> {
        let ns = u32::try_from(ns).unwrap_or(0);
        self.doc
            .create_element(tag, ns, 0, None)
            .map_err(TreeStatus::from)
    }

    /// `lxb_html_tree_create_element_for_token`, its attributes adjusted
    /// by `adjust` (Lexbor's `tree->before_append_attr`).
    pub(crate) fn create_element_for_token(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        tok: &Tok,
        ns: usize,
        adjust: Option<AttrAdjust>,
    ) -> Result<NodeId, TreeStatus> {
        let el = self.create_element_node(tok.tag_id, ns)?;
        match tok.base {
            None => self.append_attributes(tkz, el, tok, ns, adjust)?,
            Some(from) => self.append_attributes_from_element(el, from, adjust)?,
        }
        if let Some(offset) = self.created_offset(tok) {
            self.doc.set_source_offset(el, offset);
        }
        Ok(el)
    }

    /// `lxb_html_tree_append_attributes`.
    pub(crate) fn append_attributes(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        el: NodeId,
        tok: &Tok,
        ns: usize,
        adjust: Option<AttrAdjust>,
    ) -> Result<(), TreeStatus> {
        for ta in tok.attrs(tkz) {
            let Some(name) = ta.name else { continue };
            if self.attr_by_local_name_ns(el, name, ns) {
                continue;
            }
            let mut spec = AttrSpec {
                local: name,
                ns,
                prefix: 0,
                written: 0,
            };
            if let Some(adjust) = adjust {
                self.adjust(&mut spec, adjust)?;
            }
            let value = ta
                .value_begin
                .map(|_| tkz.attr_value(ta).unwrap_or_default());
            let attr = self.make_attr(spec, value.map(AttrValue::Bytes))?;
            self.append_attr_with_steps(el, attr)?;
        }
        Ok(())
    }

    /// Make an attribute: with no value - Lexbor's NULL - when `value` is
    /// none.
    fn make_attr(
        &mut self,
        spec: AttrSpec,
        value: Option<AttrValue<'_>>,
    ) -> Result<NodeId, TreeStatus> {
        let ns = u32::try_from(spec.ns).unwrap_or(0);
        let attr = self.doc.create_attr_by_ids(
            spec.local,
            ns,
            spec.prefix,
            spec.written,
            value.unwrap_or(AttrValue::Bytes(b"")),
        )?;
        if value.is_none() {
            self.valueless.insert(attr).map_err(|()| TreeStatus::Oom)?;
        }
        Ok(attr)
    }

    /// `lxb_dom_element_attr_is_exist`: the first attribute of `el` whose
    /// local name is `name` or whose qualified name as written is.
    pub(crate) fn attr_is_exist(&self, el: NodeId, name: &[u8]) -> Option<NodeId> {
        let d = &*self.doc;
        let id = d.names.attr_id(name);
        let mut a = d.first_attr(el);
        while let Some(x) = a {
            if Some(d.local(x) as usize) == id || d.attr_written(x) == Some(name) {
                return Some(x);
            }
            a = d.next_attr(x);
        }
        None
    }

    /// `lxb_dom_element_attr_by_local_name_ns_data`: whether `el` has an
    /// attribute with local name `name` in `ns`, or whose qualified name is
    /// `name`.
    fn attr_by_local_name_ns(&self, el: NodeId, name: usize, ns: usize) -> bool {
        let d = &*self.doc;
        let text = d.names.attr_name(name);
        let mut a = d.first_attr(el);
        while let Some(x) = a {
            if d.local(x) as usize == name && d.ns(x) as usize == ns {
                return true;
            }
            if text.is_some() && d.attr_written(x) == text {
                return true;
            }
            a = d.next_attr(x);
        }
        false
    }

    /// `lxb_html_tree_append_attributes_from_element`.
    fn append_attributes_from_element(
        &mut self,
        el: NodeId,
        from: NodeId,
        adjust: Option<AttrAdjust>,
    ) -> Result<(), TreeStatus> {
        let mut a = self.doc.first_attr(from);
        while let Some(x) = a {
            let attr = self.copy_attr(x, 0, adjust)?;
            self.append_attr_with_steps(el, attr)?;
            a = self.doc.next_attr(x);
        }
        Ok(())
    }

    /// A copy of the attribute `x`, not linked, with the prefix `prefix` and
    /// adjusted by `adjust`: valueless (Lexbor's NULL) when `x` is.
    pub(super) fn copy_attr(
        &mut self,
        x: NodeId,
        prefix: u32,
        adjust: Option<AttrAdjust>,
    ) -> Result<NodeId, TreeStatus> {
        let mut spec = AttrSpec {
            local: self.doc.local(x) as usize,
            ns: self.doc.ns(x) as usize,
            prefix,
            written: self.doc.written_id(x),
        };
        if let Some(adjust) = adjust {
            self.adjust(&mut spec, adjust)?;
        }
        let value = (!self.valueless.contains(x)).then_some(AttrValue::CopyOf(x));
        self.make_attr(spec, value)
    }

    /// The SVG element `el`'s name as the Standard's table writes it
    /// (`foreignObject`), set as its qualified name
    /// (`lxb_dom_element_qualified_name_set`; Lexbor ignores a failure).
    pub(crate) fn adjust_svg_tag_name(&mut self, el: NodeId) {
        let fixed = rt::SVG_FIXNAME
            .get(self.tag(el))
            .copied()
            .unwrap_or_default();
        if !fixed.is_empty() {
            let _ = self.doc.set_written(el, Some(fixed));
        }
    }

    /// `tree->before_append_attr`: how a foreign element's attributes are
    /// adjusted.
    fn adjust(&mut self, spec: &mut AttrSpec, adjust: AttrAdjust) -> Result<(), TreeStatus> {
        match adjust {
            AttrAdjust::Mathml => self.adjust_mathml_attributes(spec)?,
            AttrAdjust::Svg => self.adjust_svg_attributes(spec)?,
            AttrAdjust::Foreign => {}
        }
        self.adjust_foreign_attributes(spec)
    }

    /// `lxb_html_tree_adjust_mathml_attributes`.
    fn adjust_mathml_attributes(&mut self, spec: &mut AttrSpec) -> Result<(), TreeStatus> {
        if self.doc.names.attr_name(spec.local) == Some(&b"definitionurl"[..]) {
            spec.written = self.doc.names.qualified_append(b"definitionURL")?;
        }
        Ok(())
    }

    /// `lxb_html_tree_adjust_svg_attributes`.
    fn adjust_svg_attributes(&mut self, spec: &mut AttrSpec) -> Result<(), TreeStatus> {
        let Some(name) = self.doc.names.attr_name(spec.local) else {
            return Ok(());
        };
        if let Some((_, to)) = rt::SVG_ATTR_ADJUST.iter().find(|(from, _)| *from == name) {
            spec.written = self.doc.names.qualified_append(to)?;
        }
        Ok(())
    }

    /// `lxb_html_tree_adjust_foreign_attributes`.
    fn adjust_foreign_attributes(&mut self, spec: &mut AttrSpec) -> Result<(), TreeStatus> {
        let Some(name) = self.doc.names.attr_name(spec.local) else {
            return Ok(());
        };
        let Some(&(full, prefix, local, ns)) =
            rt::FOREIGN_ATTR_ADJUST.iter().find(|(n, ..)| *n == name)
        else {
            return Ok(());
        };
        if !prefix.is_empty() {
            let names = &mut self.doc.names;
            spec.written = names.qualified_append(full)?;
            spec.local = names.attr_local_name_append(local)?;
            spec.prefix = self.doc.intern_prefix(prefix)?.unwrap_or(0);
        }
        spec.ns = ns;
        Ok(())
    }

    /// `lxb_html_tree_insert_character`: the token's text, as it is.
    pub(crate) fn insert_character(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        tok: &Tok,
    ) -> Result<(), TreeStatus> {
        self.insert_character_for_data(tok.text(tkz))
    }

    /// `lxb_html_tree_insert_character_for_data`: appended to the text node
    /// at the place, or a new one.
    pub(crate) fn insert_character_for_data(&mut self, data: &[u8]) -> Result<(), TreeStatus> {
        let (pos, ipos) = self.appropriate_place(None).ok_or(TreeStatus::Error)?;
        if self.node_is(pos, res::TAG__DOCUMENT) {
            return Ok(());
        }
        let prev = match ipos {
            Ipos::Before => self.doc.prev(pos),
            Ipos::Child => self.doc.last_child(pos),
        };
        if let Some(p) = prev.filter(|&p| self.tag(p) == res::TAG__TEXT) {
            return self.doc.append_text(p, data).map_err(TreeStatus::from);
        }
        let name = NodeName::html(res::TAG__TEXT);
        let text = self.doc.create_data(NodeType::Text, name, data)?;
        self.insert_node(pos, text, ipos);
        Ok(())
    }

    /// `lxb_html_tree_insert_comment`: at `pos` when given, else at the
    /// appropriate place. The comment takes the namespace of where it goes.
    pub(crate) fn insert_comment(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        tok: &Tok,
        pos: Option<NodeId>,
    ) -> Result<NodeId, TreeStatus> {
        let (pos, ipos) = match pos {
            Some(p) => (p, Ipos::Child),
            None => self.appropriate_place(None).ok_or(TreeStatus::Error)?,
        };
        let name = NodeName {
            local: tok.tag_id,
            ns: self.doc.ns(pos),
            prefix: 0,
        };
        let c = self
            .doc
            .create_data(NodeType::Comment, name, tok.text(tkz))?;
        self.insert_node(pos, c, ipos);
        Ok(c)
    }

    /// `lxb_html_tree_insert_processing_instruction`, the target and data as
    /// the tokenizer splits them ([`Tokenizer::pi_parts`]).
    pub(crate) fn insert_processing_instruction(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
        pos: Option<NodeId>,
    ) -> Result<NodeId, TreeStatus> {
        let (pos, ipos) = self.appropriate_place(pos).ok_or(TreeStatus::Error)?;
        let (target, data) = tkz.pi_parts();
        let pi = self.doc.create_pi(target, data)?;
        self.insert_node(pos, pi, ipos);
        Ok(pi)
    }

    /// `lxb_html_tree_create_document_type_from_token` with
    /// `lxb_html_token_doctype_parse`: the name is the first attribute's,
    /// then a `public` or `system` attribute's value, and a system one after
    /// a public one. Returns the node and its name id (0 for none).
    pub(crate) fn create_document_type_from_token(
        &mut self,
        tkz: &Tokenizer<'_, '_>,
    ) -> Result<(NodeId, usize, usize, usize), TreeStatus> {
        let parts = tkz.doctype();
        let name_id = parts.name.and_then(|a| a.name).unwrap_or(0);
        let public = parts.public_id.unwrap_or_default();
        let system = parts.system_id.unwrap_or_default();
        /* `lxb_dom_document_type_name`: `LXB_DOM_ATTR__UNDEF` reads as "". */
        let name = match name_id {
            res::ATTR__UNDEF => &b""[..],
            id => self.doc.names.attr_name(id).unwrap_or_default(),
        };
        /* `lexbor_str_t` copies: the name is the interned attribute name. */
        let name = crate::falloc::try_to_vec(name).ok_or(TreeStatus::Oom)?;
        let names = NodeName::html(res::TAG__EM_DOCTYPE);
        let node = self.doc.create_doctype(names, &name, public, system)?;
        Ok((node, name_id, public.len(), system.len()))
    }

    /// `lxb_html_tree_generic_rawtext_parsing`.
    pub(crate) fn generic_rawtext_parsing(
        &mut self,
        tkz: &mut Tokenizer<'_, '_>,
        tok: &Tok,
    ) -> Result<NodeId, TreeStatus> {
        let el = self.insert_html_element(tkz, tok)?;
        tkz.enter_text_content(TextContent::Rawtext, tok.tag_id);
        self.original_mode = self.mode;
        self.mode = Mode::Text;
        Ok(el)
    }

    /// `lxb_html_tree_generic_rcdata_parsing`.
    pub(crate) fn generic_rcdata_parsing(
        &mut self,
        tkz: &mut Tokenizer<'_, '_>,
        tok: &Tok,
    ) -> Result<NodeId, TreeStatus> {
        let el = self.insert_html_element(tkz, tok)?;
        tkz.enter_text_content(TextContent::Rcdata, tok.tag_id);
        self.original_mode = self.mode;
        self.mode = Mode::Text;
        Ok(el)
    }

    /// `lxb_html_tree_generate_implied_end_tags`.
    pub(crate) fn generate_implied_end_tags(&mut self, ex_tag: usize, ex_ns: usize) {
        while let Some(node) = self.current_node() {
            match self.tag(node) {
                res::TAG_DD
                | res::TAG_DT
                | res::TAG_LI
                | res::TAG_OPTGROUP
                | res::TAG_OPTION
                | res::TAG_P
                | res::TAG_RB
                | res::TAG_RP
                | res::TAG_RT
                | res::TAG_RTC => {
                    if self.tag(node) == ex_tag && self.ns(node) == ex_ns {
                        return;
                    }
                    self.open_pop();
                }
                _ => return,
            }
        }
    }

    /// `lxb_html_tree_generate_all_implied_end_tags_thoroughly`.
    pub(crate) fn generate_all_implied_end_tags_thoroughly(&mut self, ex_tag: usize, ex_ns: usize) {
        while let Some(node) = self.current_node() {
            match self.tag(node) {
                res::TAG_CAPTION
                | res::TAG_COLGROUP
                | res::TAG_DD
                | res::TAG_DT
                | res::TAG_LI
                | res::TAG_OPTGROUP
                | res::TAG_OPTION
                | res::TAG_P
                | res::TAG_RB
                | res::TAG_RP
                | res::TAG_RT
                | res::TAG_RTC
                | res::TAG_TBODY
                | res::TAG_TD
                | res::TAG_TFOOT
                | res::TAG_TH
                | res::TAG_THEAD
                | res::TAG_TR => {
                    if self.tag(node) == ex_tag && self.ns(node) == ex_ns {
                        return;
                    }
                    self.open_pop();
                }
                _ => return,
            }
        }
    }

    /// `lxb_html_tree_reset_insertion_mode_appropriately`.
    pub(crate) fn reset_insertion_mode_appropriately(&mut self) {
        let mut idx = self.open.len();
        let mut last = false;
        while idx != 0 {
            idx -= 1;
            let mut node = self.open[idx];
            if idx == 0 {
                last = true;
                if let Some(f) = self.fragment {
                    node = f;
                }
            }
            /* Step 16 */
            if self.ns(node) != res::NS_HTML {
                if last {
                    self.mode = Mode::InBody;
                    return;
                }
                continue;
            }
            match self.tag(node) {
                res::TAG_TD | res::TAG_TH => {
                    if !last {
                        self.mode = Mode::InCell;
                        return;
                    }
                }
                res::TAG_TR => {
                    self.mode = Mode::InRow;
                    return;
                }
                res::TAG_TBODY | res::TAG_TFOOT | res::TAG_THEAD => {
                    self.mode = Mode::InTableBody;
                    return;
                }
                res::TAG_CAPTION => {
                    self.mode = Mode::InCaption;
                    return;
                }
                res::TAG_COLGROUP => {
                    self.mode = Mode::InColumnGroup;
                    return;
                }
                res::TAG_TABLE => {
                    self.mode = Mode::InTable;
                    return;
                }
                res::TAG_TEMPLATE => {
                    /* Lexbor asserts a template mode is there. */
                    if let Some(&m) = self.template_modes.last() {
                        self.mode = m;
                    }
                    return;
                }
                res::TAG_HEAD => {
                    if !last {
                        self.mode = Mode::InHead;
                        return;
                    }
                }
                res::TAG_BODY => {
                    self.mode = Mode::InBody;
                    return;
                }
                res::TAG_FRAMESET => {
                    self.mode = Mode::InFrameset;
                    return;
                }
                res::TAG_HTML => {
                    self.mode = if self.head.is_none() {
                        Mode::BeforeHead
                    } else {
                        Mode::AfterHead
                    };
                    return;
                }
                _ => {}
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
        }
    }

    /// `lxb_html_tree_element_in_scope`.
    pub(crate) fn element_in_scope(&self, tag: usize, ns: usize, cat: u8) -> Option<NodeId> {
        for &node in self.open.iter().rev() {
            if self.tag(node) == tag && self.ns(node) == ns {
                return Some(node);
            }
            if is_category(self.tag(node), self.ns(node), cat) {
                return None;
            }
        }
        None
    }

    /// `lxb_html_tree_element_in_scope_by_node`.
    pub(crate) fn element_in_scope_by_node(&self, by: NodeId, cat: u8) -> Option<NodeId> {
        for &node in self.open.iter().rev() {
            if node == by {
                return Some(node);
            }
            if is_category(self.tag(node), self.ns(node), cat) {
                return None;
            }
        }
        None
    }

    /// The `lxb_html_tree_element_in_scope_*` family over a set of tags.
    fn element_in_scope_any(&self, tags: impl Fn(usize) -> bool, cat: u8) -> Option<NodeId> {
        for &node in self.open.iter().rev() {
            if tags(self.tag(node)) && self.ns(node) == res::NS_HTML {
                return Some(node);
            }
            if is_category(self.tag(node), self.ns(node), cat) {
                return None;
            }
        }
        None
    }

    /// `lxb_html_tree_element_in_scope_h123456`.
    pub(crate) fn element_in_scope_h123456(&self) -> Option<NodeId> {
        self.element_in_scope_any(super::stacks::is_h123456, rt::CAT_SCOPE)
    }

    /// `lxb_html_tree_element_in_scope_tbody_thead_tfoot`.
    pub(crate) fn element_in_scope_tbody_thead_tfoot(&self) -> Option<NodeId> {
        self.element_in_scope_any(
            |t| matches!(t, res::TAG_TBODY | res::TAG_THEAD | res::TAG_TFOOT),
            rt::CAT_SCOPE_TABLE,
        )
    }

    /// `lxb_html_tree_element_in_scope_td_th`.
    pub(crate) fn element_in_scope_td_th(&self) -> Option<NodeId> {
        self.element_in_scope_any(
            |t| matches!(t, res::TAG_TD | res::TAG_TH),
            rt::CAT_SCOPE_TABLE,
        )
    }

    /// `lxb_html_tree_element_in_scope_option_optgroup`.
    pub(crate) fn element_in_scope_option_optgroup(&self) -> Option<NodeId> {
        self.element_in_scope_any(
            |t| matches!(t, res::TAG_OPTION | res::TAG_OPTGROUP),
            rt::CAT_SCOPE,
        )
    }

    /// `lxb_html_tree_check_scope_element`.
    pub(crate) fn check_scope_element(&self) -> bool {
        self.open.iter().any(|&n| {
            matches!(
                self.tag(n),
                res::TAG_DD
                    | res::TAG_DT
                    | res::TAG_LI
                    | res::TAG_OPTGROUP
                    | res::TAG_OPTION
                    | res::TAG_P
                    | res::TAG_RB
                    | res::TAG_RP
                    | res::TAG_RT
                    | res::TAG_RTC
                    | res::TAG_TBODY
                    | res::TAG_TD
                    | res::TAG_TFOOT
                    | res::TAG_TH
                    | res::TAG_THEAD
                    | res::TAG_TR
                    | res::TAG_BODY
                    | res::TAG_HTML
            )
        })
    }

    /// `lxb_html_tree_close_p_element`.
    pub(crate) fn close_p_element(&mut self, tok: &Tok) {
        self.close_element(tok, res::TAG_P, Some(res::TAG_P), E::Unelinopelst);
    }

    /// Whether the current node is the HTML element `tag`.
    #[inline]
    pub(crate) fn current_is(&self, tag: usize) -> bool {
        self.current_node().is_some_and(|n| self.node_is(n, tag))
    }

    /// The Standard's recurring close of an element: generate implied end
    /// tags, except for the HTML element `except`; report `err` unless the
    /// current node is then `tag`; pop up to and including `tag`.
    pub(crate) fn close_element(&mut self, tok: &Tok, tag: usize, except: Option<usize>, err: E) {
        match except {
            Some(t) => self.generate_implied_end_tags(t, res::NS_HTML),
            None => self.generate_implied_end_tags(res::TAG__UNDEF, res::NS__UNDEF),
        }
        if !self.current_is(tag) {
            self.parse_error(tok, err);
        }
        self.open_pop_until_tag_id(tag, res::NS_HTML, true);
    }

    /// `lxb_html_tree_acknowledge_token_self_closing`.
    pub(crate) fn acknowledge_token_self_closing(&mut self, tok: &Tok) {
        if tok.flags & flags::CLOSE_SELF == 0 {
            return;
        }
        if !is_void(tok.tag_id) {
            self.parse_error(tok, E::Novohtelsttawitrso);
        }
    }

    /// `lxb_html_tree_mathml_text_integration_point`.
    pub(crate) fn mathml_text_integration_point(&self, n: NodeId) -> bool {
        self.ns(n) == res::NS_MATH
            && matches!(
                self.tag(n),
                res::TAG_MI | res::TAG_MO | res::TAG_MN | res::TAG_MS | res::TAG_MTEXT
            )
    }

    /// `lxb_html_tree_html_integration_point`.
    pub(crate) fn html_integration_point(&self, n: NodeId) -> bool {
        if self.context_integration_point && self.fragment == Some(n) {
            return true;
        }
        if self.ns(n) == res::NS_MATH && self.tag(n) == res::TAG_ANNOTATION_XML {
            /* `lxb_dom_element_attr_is_exist(element, "encoding")`: by local
             * name, or by qualified name. */
            let Some(x) = self.attr_is_exist(n, b"encoding") else {
                return false;
            };
            if self.valueless.contains(x) {
                return false;
            }
            let v = self.doc.attr_value(x).unwrap_or_default();
            return v.eq_ignore_ascii_case(b"text/html")
                || v.eq_ignore_ascii_case(b"application/xhtml+xml");
        }
        self.ns(n) == res::NS_SVG
            && matches!(
                self.tag(n),
                res::TAG_FOREIGNOBJECT | res::TAG_DESC | res::TAG_TITLE
            )
    }

    /// `lxb_html_tree_adoption_agency_algorithm`: `true` when the caller is
    /// to act as "any other end tag" (Lexbor's return), `Err` on failure.
    pub(crate) fn adoption_agency_algorithm(&mut self, tok: &Tok) -> Result<bool, TreeStatus> {
        let subject = tok.tag_id;
        /* State 2 */
        let Some(node) = self.current_node() else {
            return Ok(false);
        };
        if self.node_is(node, subject) && self.afe_find_by_node_reverse(node).is_none() {
            self.open_pop();
            return Ok(false);
        }
        /* State 3-4 */
        let mut outer_loop = 0;
        while outer_loop < 8 {
            /* State 5 */
            outer_loop += 1;

            /* State 6 */
            let mut formatting = None;
            for idx in (0..self.afe.len()).rev() {
                match self.afe[idx] {
                    None => return Ok(true),
                    Some(n) if self.tag(n) == subject => {
                        formatting = Some((idx, n));
                        break;
                    }
                    Some(_) => {}
                }
            }
            let Some((formatting_index, formatting_element)) = formatting else {
                return Ok(true);
            };

            /* State 7 */
            let Some(oel_formatting_idx) = self.open_find_by_node_reverse(formatting_element)
            else {
                self.parse_error(tok, E::Mielinopelst);
                self.afe_remove_by_node(formatting_element);
                return Ok(false);
            };

            /* State 8 */
            if self
                .element_in_scope_by_node(formatting_element, rt::CAT_SCOPE)
                .is_none()
            {
                self.parse_error(tok, E::Mielinsc);
                return Ok(false);
            }

            /* State 9 */
            if self.current_node() != Some(formatting_element) {
                self.parse_error(tok, E::Unelinopelst);
            }

            /* State 10 */
            let furthest = (oel_formatting_idx..self.open.len()).find(|&i| {
                let n = self.open[i];
                is_category(self.tag(n), self.ns(n), rt::CAT_SPECIAL)
            });

            /* State 11 */
            let Some(mut furthest_block_idx) = furthest else {
                self.open_pop_until_node(formatting_element, true);
                self.afe_remove_by_node(formatting_element);
                return Ok(false);
            };
            let furthest_block = self.open[furthest_block_idx];

            /* State 12 */
            let Some(common_ancestor) = oel_formatting_idx
                .checked_sub(1)
                .and_then(|i| self.open_get(i))
            else {
                return Ok(false);
            };

            /* State 13. The bookmark and the formatting element's index are
             * positions in a list the inner loop removes from: the Standard's
             * bookmark stays between its neighbours, so both move down with
             * an entry removed before them. Lexbor kept the indices as they
             * were. */
            let mut bookmark = formatting_index;
            let mut formatting_index = formatting_index;

            /* State 14 */
            let mut last = furthest_block;
            let mut node_idx = furthest_block_idx;
            let mut inner_loop_counter = 0;
            loop {
                inner_loop_counter += 1;
                /* State 14.3 */
                if node_idx == 0 {
                    return Ok(false);
                }
                node_idx -= 1;
                let mut node = self.open[node_idx];

                /* State 14.4 */
                if node == formatting_element {
                    break;
                }

                /* State 14.5 */
                let mut afe_node_idx = self.afe_find_by_node_reverse(node);
                if inner_loop_counter > 3 {
                    if let Some(i) = afe_node_idx {
                        self.afe_remove(i);
                        if compat::fixed(Fix::P11) {
                            if i < formatting_index {
                                formatting_index -= 1;
                            }
                            if i < bookmark {
                                bookmark -= 1;
                            }
                        }
                    }
                    afe_node_idx = None;
                }

                /* State 14.6 */
                let Some(afe_node_idx) = afe_node_idx else {
                    self.open_remove_by_node(node);
                    continue;
                };

                /* State 14.7 */
                let mut fake = Tok::fake(self.tag(node));
                fake.base = Some(node);
                let el = self.create_element_for_fake(&fake)?;
                node = el;
                self.afe[afe_node_idx] = Some(node);
                self.open[node_idx] = node;

                /* State 14.8 */
                if last == furthest_block {
                    bookmark = afe_node_idx + 1;
                }

                /* State 14.9 */
                if self.doc.parent(last).is_some() {
                    self.doc.detach(last);
                }
                self.doc.append_child(node, last);

                /* State 14.10 */
                last = node;
            }

            if self.doc.parent(last).is_some() {
                self.doc.detach(last);
            }

            /* State 15 */
            let Some((pos, ipos)) = self.appropriate_place(Some(common_ancestor)) else {
                return Ok(false);
            };
            self.insert_node(pos, last, ipos);

            /* State 16 */
            let mut fake = Tok::fake(self.tag(formatting_element));
            fake.base = Some(formatting_element);
            let element = self.create_element_for_fake(&fake)?;

            /* State 17 */
            let mut child = self.doc.first_child(furthest_block);
            while let Some(c) = child {
                let next = self.doc.next(c);
                self.doc.detach(c);
                self.doc.append_child(element, c);
                child = next;
            }

            /* State 18 */
            self.doc.append_child(furthest_block, element);

            /* State 19 */
            self.afe_remove(formatting_index);
            if formatting_index < bookmark && compat::fixed(Fix::P11) {
                bookmark -= 1;
            }
            if bookmark > self.afe.len() {
                bookmark = self.afe.len();
            }
            self.afe_insert(element, bookmark)?;

            /* State 20 */
            self.open_remove_by_node(formatting_element);
            furthest_block_idx = self.open_find_by_node(furthest_block).unwrap_or(0);
            self.open_insert_after(element, furthest_block_idx)?;
        }
        Ok(false)
    }

    /// `lxb_html_tree_create_element_for_token` for a token with a base
    /// element (no tokenizer attributes), in the HTML namespace.
    fn create_element_for_fake(&mut self, fake: &Tok) -> Result<NodeId, TreeStatus> {
        let el = self.create_element_node(fake.tag_id, res::NS_HTML)?;
        if let Some(from) = fake.base {
            self.append_attributes_from_element(el, from, None)?;
        }
        Ok(el)
    }
}
