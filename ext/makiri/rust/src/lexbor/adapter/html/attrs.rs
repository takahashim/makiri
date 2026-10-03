//! The DOM Standard's attribute algorithms (§4.9), spelled once over Lexbor's
//! attribute list. Every attribute read by name and every attribute write in
//! Makiri comes through here.
//!
//! Lexbor's own element helpers are HTML-parser conveniences, not these
//! algorithms, and two of their properties are hazards to a DOM caller:
//!
//! - **Local-name matching.** `lxb_dom_element_attr_is_exist` /
//!   `attr_by_name` compare Lexbor's lower-cased LOCAL name, ignoring the
//!   namespace, so `el["href"]` on `<a xlink:href>` found - and a set
//!   overwrote - the xlink attribute. The DOM's by-name family keys on the
//!   QUALIFIED name, and the by-namespace family on (namespace, local name).
//! - **Free on append.** `lxb_dom_element_attr_append` keeps the
//!   `element->attr_id` / `attr_class` shortcuts (what `#id` / `.class` CSS
//!   matching and Lexbor's id lookups read) by DESTROYING whatever attribute
//!   the shortcut held when another one with Lexbor's local name `id` /
//!   `class` arrives - `setAttributeNS(nil, "ID")`, a namespaced `id`, an
//!   XHTML `class`. A Ruby `Attr` wrapper can alias the destroyed attribute.
//!
//! So [`HtmlElement::link_attr`] hands Lexbor's append an element whose two
//! shortcuts are EMPTY - with nothing to replace, it frees nothing - and then
//! sets them itself to the DOM's view: the no-namespace attribute whose local
//! name is exactly `id` / `class`, if any. Removal is Lexbor's
//! `lxb_dom_element_attr_remove`, which only unlinks (and clears a shortcut
//! naming the attribute); nothing here destroys an attribute.
//!
//! One Lexbor path still appends by its own rules: `importNode`'s attribute
//! copy. [`repair_import`] walks the copy beside its source afterwards and puts
//! back what that copy dropped - see there.

#![allow(unsafe_code)]

use super::*;
use crate::falloc::OomOption;

/// Which of Lexbor's two attribute shortcuts an attribute is, in the DOM's
/// terms: the element's ID (no namespace, local name exactly `id`) or its
/// class attribute (the same, `class`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Id,
    Class,
}

impl Shortcut {
    /// The shortcut a NO-NAMESPACE attribute with this local name is.
    fn named(local: &[u8]) -> Option<Shortcut> {
        match local {
            b"id" => Some(Shortcut::Id),
            b"class" => Some(Shortcut::Class),
            _ => None,
        }
    }
}

/// Whether `qualified` is `name` ASCII-lowercased, without making the copy.
fn eq_lowered(qualified: &[u8], name: &[u8]) -> bool {
    qualified.len() == name.len()
        && qualified
            .iter()
            .zip(name)
            .all(|(&q, &n)| q == n.to_ascii_lowercase())
}

impl<'doc> HtmlElement<'doc> {
    /// Whether this is an HTML element in an HTML document - the DOM's
    /// condition for ASCII-lowercasing a by-name lookup or a set's name.
    fn is_html_in_html_doc(self) -> bool {
        self.node().ns_id() == Some(NsId::HTML) && self.node().owner_document().is_html_document()
    }

    /// DOM "get an attribute by name": the first attribute whose QUALIFIED
    /// name is `qname` - ASCII-lowercased first for an HTML element in an HTML
    /// document. A prefixed `xlink:href` is not found as `href`.
    pub fn attr_by_name(self, qname: &[u8]) -> Option<HtmlAttr<'doc>> {
        if self.is_html_in_html_doc() {
            self.attrs().find(|a| eq_lowered(a.qualified_name(), qname))
        } else {
            self.attrs().find(|a| a.qualified_name() == qname)
        }
    }

    /// [`attr_by_name`](Self::attr_by_name) with `qname` resolved once
    /// ([`HtmlDoc::resolve_attr_name`]): an attribute whose local-name id
    /// differs is passed over without reading its name, and one that shares
    /// it is confirmed by exactly `attr_by_name`'s comparison - so the answer
    /// is `attr_by_name`'s.
    ///
    /// `self` must be an element of the document `name` was resolved in, as
    /// every node a walking CSS query reaches is: that is not read from the
    /// element, which would cost a document read per candidate. Another
    /// document's element would be answered from the wrong table - a wrong
    /// answer, not an unsound one (ids are plain integers) - and a debug
    /// build asserts it cannot happen.
    pub fn attr_by_resolved_name(self, qname: &[u8], name: AttrName) -> Option<HtmlAttr<'doc>> {
        let Some(doc) = name.doc else {
            return self.attr_by_name(qname);
        };
        debug_assert!(self.node().owner_document().raw == doc);
        let id = name.id?;
        let html = name.html_doc && self.node().ns_id() == Some(NsId::HTML);
        self.attrs().find(|a| {
            a.local_id() == id
                // Named by its lower-cased local name, which is `qname`'s
                // lower-cased form (the id says so): `qname` itself when the
                // lookup lower-cases or `qname` has no upper case.
                && (a.named_by_local() && (html || name.lower)
                    || if html {
                        eq_lowered(a.qualified_name(), qname)
                    } else {
                        a.qualified_name() == qname
                    })
        })
    }

    /// DOM "get an attribute by namespace and local name". `ns` is the
    /// attribute's OWN namespace ([`HtmlAttr::own_ns`]), `None` for none; the
    /// local name is compared case-preserved and case-sensitively.
    pub fn attr_by_ns(self, ns: Option<NsId>, local: &[u8]) -> Option<HtmlAttr<'doc>> {
        self.attrs()
            .find(|a| a.own_ns() == ns && a.dom_local_name() == local)
    }

    /// The element's ID: its no-namespace attribute whose local name is
    /// exactly `id`, read from Lexbor's `attr_id` shortcut (which
    /// [`link_attr`](Self::link_attr) keeps pointing at exactly that) - what
    /// Lexbor's own `#id` matching reads. Unlike `get_attribute(b"id")` it
    /// does not find an unprefixed `id` set IN a namespace.
    #[inline]
    pub fn id_attr(self) -> Option<HtmlAttr<'doc>> {
        // SAFETY: a live element; the shortcut is null or one of its own
        // attributes, live for 'doc.
        HtmlAttr::link(unsafe { (*self.raw()).attr_id })
    }

    /// As [`id_attr`](Self::id_attr), for the class attribute
    /// (`attr_class`).
    #[inline]
    pub fn class_attr(self) -> Option<HtmlAttr<'doc>> {
        // SAFETY: as `id_attr`.
        HtmlAttr::link(unsafe { (*self.raw()).attr_class })
    }

    /// DOM `hasAttribute(qname)`.
    pub fn has_attribute(self, qname: &[u8]) -> bool {
        self.attr_by_name(qname).is_some()
    }
    /// DOM `getAttribute(qname)`: the value, or None when there is no such
    /// attribute (an attribute with no value answers `Some(b"")`).
    pub fn get_attribute(self, qname: &[u8]) -> Option<&'doc [u8]> {
        self.attr_by_name(qname).map(HtmlAttr::value)
    }

    /* The writing steps. Reached only through the two clearance types -
     * `HtmlElementMut` (a receiver cleared for editing) and `BuildingElement`
     * (an element no tree holds yet) - which differ in who may call them, not
     * in what is done. */

    /// DOM "append an attribute", without letting Lexbor free anything.
    ///
    /// Lexbor's append replaces - and destroys - the attribute a shortcut
    /// holds when `at`'s Lexbor local name is `id` / `class`. With both
    /// shortcuts emptied first there is nothing to replace; they are then put
    /// back, and `at` takes `shortcut` - which the caller passes when `at` is
    /// the DOM's ID / class attribute. The callers keep "at most one attribute
    /// per (namespace, local name)", so `at` never displaces a shortcut still
    /// in the list.
    ///
    /// # Safety
    /// The element may be changed by the caller, and `at` is a live attribute
    /// of the same document that no element holds.
    unsafe fn link_attr(self, at: HtmlAttr<'doc>, shortcut: Option<Shortcut>) {
        let el = self.raw();
        // SAFETY: per the contract; Lexbor's append reads and writes only the
        // element's attribute list, the two shortcuts and `at`'s links, and
        // runs the document's attribute-append hook, which reads the name and
        // value it is given.
        unsafe {
            let (id, class) = ((*el).attr_id, (*el).attr_class);
            (*el).attr_id = core::ptr::null_mut();
            (*el).attr_class = core::ptr::null_mut();
            /* The status is the append hook's; the attribute is linked
             * whatever it says, as it always was. */
            let _ = lxb::lxb_dom_element_attr_append(el, at.raw());
            (*el).attr_id = id;
            (*el).attr_class = class;
            match shortcut {
                Some(Shortcut::Id) => (*el).attr_id = at.raw(),
                Some(Shortcut::Class) => (*el).attr_class = at.raw(),
                None => {}
            }
        }
    }

    /// Create an attribute of this element's document named `qname` in `ns`
    /// (`None` = no namespace), with `value`, not yet linked.
    ///
    /// A no-namespace name is stored case-preserved, or lower-cased when
    /// `lower` (setAttribute on an HTML element in an HTML document), and in
    /// the element's own namespace - Lexbor's (and its parser's) spelling of
    /// "none", which [`HtmlAttr::own_ns`] reads back as `None`.
    ///
    /// `Err` when any step failed; the unlinked attribute is left for the
    /// arena to reclaim wholesale, the "never destroy" convention.
    fn create_attr(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
        lower: bool,
    ) -> Result<HtmlAttr<'doc>, AdapterOom> {
        /* Interned as written first: Lexbor's set_name_ns below interns the
         * URI too, case-folded, and its id is replaced by this one. */
        let ns_id = match ns {
            Some(uri) => Some(self.node().owner_document().intern_ns(uri).or_oom()?),
            None => None,
        };
        // SAFETY: a live element of a live document; every slice is read and
        // copied by Lexbor, and the attribute is one nothing else holds.
        unsafe {
            let at = lxb::lxb_dom_attr_interface_create(self.node().owner_document().as_raw());
            let at = HtmlAttr::link(at).or_oom()?;
            let named = match (ns, ns_id) {
                (Some(uri), Some(id)) => {
                    let st = lxb::lxb_dom_attr_set_name_ns(
                        at.raw(),
                        uri.as_ptr(),
                        uri.len(),
                        qname.as_ptr(),
                        qname.len(),
                        false,
                    );
                    (*at.raw()).node.ns = id.raw();
                    st
                }
                _ => {
                    (*at.raw()).node.ns = (*self.raw()).node.ns;
                    lxb::lxb_dom_attr_set_name(at.raw(), qname.as_ptr(), qname.len(), lower)
                }
            };
            lexbor_ok(named)?;
            at.set_value(value)?;
            Ok(at)
        }
    }

    /// DOM `setAttribute(qname, value)`: change the attribute
    /// [`attr_by_name`](Self::attr_by_name) finds, or append a new
    /// no-namespace one named `qname` (lower-cased for an HTML element in an
    /// HTML document). `Err` when Lexbor could not store it, in which case an
    /// existing attribute keeps its value.
    ///
    /// # Safety
    /// The element may be changed by the caller.
    pub(super) unsafe fn set_attribute_value(
        self,
        qname: &[u8],
        value: &[u8],
    ) -> Result<HtmlAttr<'doc>, AdapterOom> {
        if let Some(at) = self.attr_by_name(qname) {
            return at.set_value(value).map(|()| at);
        }
        let at = self.create_attr(None, qname, value, self.is_html_in_html_doc())?;
        // SAFETY: per the contract; `at` was just made and is unlinked, with
        // no namespace.
        unsafe { self.link_attr(at, Shortcut::named(at.dom_local_name())) };
        Ok(at)
    }

    /// DOM `setAttributeNS(ns, qname, value)`: change the attribute
    /// [`attr_by_ns`](Self::attr_by_ns) finds for (`ns`, local part of
    /// `qname`) - its prefix is kept - or append a new one named `qname`.
    /// `ns` is a namespace URI, `None` for none. `Err` when Lexbor could not
    /// store it.
    ///
    /// # Safety
    /// The element may be changed by the caller.
    pub(super) unsafe fn set_attribute_value_ns(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        let local = match qname.iter().position(|&b| b == b':') {
            Some(i) => &qname[i + 1..],
            None => qname,
        };
        /* Looked up, not interned: no attribute carries a namespace the
         * document never interned, and the create below interns it. */
        let existing = match ns {
            Some(uri) => self
                .node()
                .owner_document()
                .lookup_ns(uri)
                .and_then(|id| self.attr_by_ns(Some(id), local)),
            None => self.attr_by_ns(None, local),
        };
        if let Some(at) = existing {
            return at.set_value(value);
        }
        // SAFETY: per the contract - the element may be changed.
        unsafe { self.append_attribute(ns, qname, value) }
    }

    /// DOM "append an attribute" of a new one named `qname` in `ns` (`None` =
    /// none), case preserved, WITHOUT looking for an existing one: for a copy
    /// whose source already keeps (namespace, local name) unique. `Err` when
    /// any step failed.
    ///
    /// # Safety
    /// The element may be changed by the caller.
    pub(super) unsafe fn append_attribute(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        let at = self.create_attr(ns, qname, value, false)?;
        /* The namespace is known here, so an attribute set IN a namespace -
         * even its element's own, which `HtmlAttr::own_ns` cannot tell from
         * none afterwards - is never the ID or class attribute. */
        let shortcut = if ns.is_none() {
            Shortcut::named(at.dom_local_name())
        } else {
            None
        };
        // SAFETY: per the contract; `at` was just made and is unlinked.
        unsafe { self.link_attr(at, shortcut) };
        Ok(())
    }

    /// DOM "remove an attribute": unlink `at` from this element. The arena
    /// keeps it, like a detached node - a Ruby wrapper may still hold it.
    /// Lexbor clears a shortcut that named it.
    ///
    /// # Safety
    /// The element may be changed by the caller, and `at` is one of its
    /// attributes.
    pub(super) unsafe fn unlink_attr(self, at: HtmlAttr<'doc>) {
        // SAFETY: per the contract; Lexbor only unlinks.
        unsafe { lxb::lxb_dom_element_attr_remove(self.raw(), at.raw()) };
    }

    /// The number of attributes.
    fn attr_count(self) -> usize {
        self.attrs().count()
    }
}

/// Undo what Lexbor's `importNode` did to attributes: `dst` is the fresh copy
/// of `src` (deep or not) that `lxb_dom_document_import_node` just made.
///
/// That copy appends each attribute with Lexbor's append, so an element
/// holding two attributes with Lexbor's local name `id` (or `class`) - `id`
/// beside a namespaced or `ID`-spelled one, which the DOM allows - comes out
/// with the earlier one DESTROYED, and the shortcut on whichever came last.
/// The copy is fresh and nothing aliases it, so the destroyed attribute
/// hurts no wrapper; but the copy is wrong. Here each copied element is
/// compared with its source: one short of attributes gets its list rebuilt
/// from the source's, through [`HtmlElement::link_attr`], and every copy
/// takes its shortcuts from the source's - the attribute at the same place.
///
/// The walk is iterative and in step - `importNode` builds the copy in the
/// source's own pre-order - and fails closed if the two ever differ in shape.
/// `Err` on an allocation failure or a mismatch; the copy is then abandoned
/// to the arena, like any failed import.
///
/// # Safety
/// `dst` is a copy nothing but the caller holds, in the document `doc`.
pub(super) unsafe fn repair_import(
    doc: HtmlDoc<'_>,
    src: HtmlNode<'_>,
    dst: HtmlNode<'_>,
    deep: bool,
) -> Result<(), AdapterOom> {
    let (mut s, mut d) = (src, dst);
    loop {
        if s.node_type() != d.node_type() {
            return Err(AdapterOom);
        }
        if let (Some(se), Some(de)) = (s.element(), d.element()) {
            // SAFETY: `de` is part of the caller's unshared copy.
            unsafe { repair_element(doc, se, de)? };
        }
        if !deep {
            return Ok(());
        }
        match (s.first_child(), d.first_child()) {
            (Some(sc), Some(dc)) => {
                (s, d) = (sc, dc);
                continue;
            }
            (None, None) => {}
            _ => return Err(AdapterOom),
        }
        /* Climb to the next sibling, stopping at the roots. */
        loop {
            if s == src {
                return if d == dst { Ok(()) } else { Err(AdapterOom) };
            }
            match (s.next(), d.next()) {
                (Some(sn), Some(dn)) => {
                    (s, d) = (sn, dn);
                    break;
                }
                (None, None) => match (s.parent(), d.parent()) {
                    (Some(sp), Some(dp)) => (s, d) = (sp, dp),
                    _ => return Err(AdapterOom),
                },
                _ => return Err(AdapterOom),
            }
        }
    }
}

/// [`repair_import`] for one element pair.
///
/// # Safety
/// `de` is an element of a copy nothing but the caller holds, in `doc`.
unsafe fn repair_element(
    doc: HtmlDoc<'_>,
    se: HtmlElement<'_>,
    de: HtmlElement<'_>,
) -> Result<(), AdapterOom> {
    let el = de.raw();
    if se.attr_count() != de.attr_count() {
        /* Lexbor destroyed some copies: unlink the survivors (fresh, so
         * nothing holds them; the arena keeps them) and copy the source's
         * list again, in order. */
        while let Some(at) = de.first_attr() {
            // SAFETY: an attribute of the caller's unshared copy.
            unsafe { de.unlink_attr(at) };
        }
        for sa in se.attrs() {
            // SAFETY: a live source attribute, only read; the clone is made in
            // `doc` and is unlinked.
            let copy = unsafe { lxb::lxb_dom_attr_interface_clone(doc.as_raw(), sa.raw()) };
            let copy = HtmlAttr::link(copy).or_oom()?;
            // SAFETY: per the contract; `copy` is unlinked. The shortcuts are
            // set below.
            unsafe { de.link_attr(copy, None) };
        }
    }
    /* Lexbor's copy re-interns a namespace past the built-in ones in this
     * document case-folded (`lxb_dom_node_interface_copy`), so `fooNamespace`
     * would arrive as `foonamespace`: such a copy gets its source's URI as
     * written, element and attributes alike. An attribute with no namespace
     * of its own carries its element's, and gets the same id the element
     * does. */
    if se.node().owner_document() != doc {
        // SAFETY: the copy is unshared, per the contract.
        unsafe {
            restore_ns(doc, se.node(), de.node())?;
            for (sa, da) in se.attrs().zip(de.attrs()) {
                restore_ns(doc, sa.node(), da.node())?;
            }
        }
    }
    /* The lists now match one for one, so each shortcut goes to the copy of
     * the attribute that holds it in the source - which the source keeps as
     * the DOM's ID / class attribute. */
    // SAFETY: the source's shortcuts are only read; the copy's are written,
    // per the contract, with attributes of its own list or null.
    unsafe {
        let (sid, sclass) = ((*se.raw()).attr_id, (*se.raw()).attr_class);
        (*el).attr_id = core::ptr::null_mut();
        (*el).attr_class = core::ptr::null_mut();
        for (sa, da) in se.attrs().zip(de.attrs()) {
            if sa.raw() == sid {
                (*el).attr_id = da.raw();
            } else if sa.raw() == sclass {
                (*el).attr_class = da.raw();
            }
        }
    }
    Ok(())
}

/// Give `dst`, a copy of `src` from another document, `src`'s namespace as
/// written - for one past the built-in ones, whose id Lexbor's copy re-interned
/// case-folded. A built-in id is the same number in every document.
///
/// # Safety
/// `dst` is a node of a copy nothing but the caller holds, in `doc`.
unsafe fn restore_ns(
    doc: HtmlDoc<'_>,
    src: HtmlNode<'_>,
    dst: HtmlNode<'_>,
) -> Result<(), AdapterOom> {
    if src.ns_id().is_none_or(|id| id.is_static()) {
        return Ok(());
    }
    let id = doc.intern_ns(src.ns_uri().or_oom()?).or_oom()?;
    // SAFETY: per the contract; `id` is interned in `doc`'s table.
    unsafe { (*dst.as_raw()).ns = id.raw() };
    Ok(())
}
