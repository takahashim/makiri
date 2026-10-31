//! The XML node's mutators and the Document factories.
//!
//! The Ruby surface of each edit: the arguments converted and checked, the
//! error each refusal raises, the value handed back. The arena itself is only
//! ever reached through [`Editing::with_arena`] or [`with_arena_for_new_node`],
//! after every argument is converted -
//! converting runs Ruby, and the closure it lends the arena to runs none - and
//! what holds two arenas at once (adopting, importing) is a primitive in
//! [`crate::bridge::xml`]. So this module holds no unsafe.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, RHash, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::bridge::string::{namespace_arg, optional};

use crate::bridge::xml::{
    begin_edit, import_copy, insert, verified_data, verified_name, verified_text,
    with_arena_for_new_node, wrap_xml_node as wrap, xml_mut_result, Editing, XmlSelf,
};
use crate::dom_rules::{self, TextContent};
use crate::glue::dom_names::dom_extract;
use crate::glue::node::DOCUMENT_NOT_REMOVABLE;
use crate::node_type::NodeType;
use crate::xml::arena::DoctypeId;
use crate::xml::dom_name::{self, split_loose_dom_name};
use crate::xml::model::{ArenaKind, Document as XmlDoc};
use crate::xml::mutate::{self, Place};
use crate::xml::qname::Split;

/* ------------------------------------------------------------------ */
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self.
pub fn remove(ruby: &crate::bridge::rb::Ruby, mut this: XmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    let kind = this.read()?.type_(this.id);
    if !dom_rules::removable(kind.map_or(NodeType::Other, NodeType::from)) {
        return Err(makiri_error(DOCUMENT_NOT_REMOVABLE));
    }
    /* An Attr leaves its element's attribute list: an attribute edit,
     * which changes no child list. */
    let attr = kind == Some(ArenaKind::Attribute);
    let edit = begin_edit(ruby, &mut this)?;
    if attr {
        edit.with_attributes(mutate::remove)?;
    } else {
        edit.with_arena(mutate::remove)?;
    }
    Ok(rb_self)
}

/// The receiver cleared for an edit, which must be an element.
fn element_for<'a>(ruby: &Ruby, this: &'a mut XmlSelf) -> Result<Editing<'a>, Error> {
    let edit = begin_edit(ruby, this)?;
    if edit.read()?.type_(edit.id()) != Some(ArenaKind::Element) {
        return Err(makiri_error(
            "cannot set an attribute on a non-element node",
        ));
    }
    Ok(edit)
}

/// `element[name] = value` -> value.
pub fn aset(ruby: &Ruby, mut this: XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    aset_on(ruby, &mut this, name, val)
}

/// [`aset`] on a receiver the caller holds, so a loop of writes converts it
/// once.
fn aset_on(ruby: &Ruby, this: &mut XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    let edit = element_for(ruby, this)?;
    let nv = verified_name(ruby, name, "attribute name")?;
    let vv = verified_data(ruby, val, "attribute value")?;
    let (name, value) = (nv.as_bytes(), vv.as_bytes());
    /* A bad name is refused before the edit is recorded. */
    xml_mut_result(mutate::checked_attribute_name(name))?;
    xml_mut_result(edit.with_attributes(|d, n| mutate::set_attribute(d, n, name, value))?)?;
    Ok(val)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    ruby: &Ruby,
    mut this: XmlSelf,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    let edit = element_for(ruby, &mut this)?;
    let qv = verified_name(ruby, qname, "attribute qualified name")?;
    let vv = verified_data(ruby, val, "attribute value")?;
    let nv = namespace_arg(ruby, ns, "namespace")?;
    let (ns, qname, value) = (
        nv.as_ref().map_or(&b""[..], |n| n.as_bytes()),
        qv.as_bytes(),
        vv.as_bytes(),
    );
    /* A bad name is refused before the edit is recorded. */
    xml_mut_result(mutate::checked_attribute_ns_name(ns, qname))?;
    xml_mut_result(edit.with_attributes(|d, n| mutate::set_attribute_ns(d, n, ns, qname, value))?)?;
    Ok(val)
}

/// `element.set_loose_dom_attribute(qualified_name, value)` -> value.
///
/// The DOM's `setAttribute` for a caller building a browser's tree over XML -
/// `create_loose_dom_element`'s counterpart: the name is not split, and a new
/// attribute is in no namespace. `ArgumentError` for a name the DOM refuses.
pub fn set_loose_dom_attribute(
    ruby: &Ruby,
    mut this: XmlSelf,
    name: Value,
    val: Value,
) -> Result<Value, Error> {
    let edit = element_for(ruby, &mut this)?;
    let nv = verified_name(ruby, name, "attribute name")?;
    let vv = verified_data(ruby, val, "attribute value")?;
    let (name, value) = (nv.as_bytes(), vv.as_bytes());
    /* A bad name is refused before the edit is recorded. */
    xml_mut_result(mutate::checked_loose_dom_attribute_name(name))?;
    xml_mut_result(
        edit.with_attributes(|d, n| mutate::set_loose_dom_attribute(d, n, name, value))?,
    )?;
    Ok(val)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    mut this: XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    let rb_self = this.value;
    let edit = begin_edit(ruby, &mut this)?;
    if edit.read()?.type_(edit.id()) != Some(ArenaKind::Element) {
        return Ok(rb_self);
    }
    let lv = verified_text(ruby, local, "attribute local name")?;
    let nv = namespace_arg(ruby, ns, "namespace")?;
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    let local = lv.as_bytes();
    edit.with_attributes(|d, n| mutate::remove_attribute_ns(d, n, ns, local))?;
    Ok(rb_self)
}

/// `element.delete(name)` / `#remove_attribute` -> self.
pub fn delete(ruby: &Ruby, mut this: XmlSelf, name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let edit = begin_edit(ruby, &mut this)?;
    if edit.read()?.type_(edit.id()) != Some(ArenaKind::Element) {
        return Ok(rb_self);
    }
    let nv = verified_text(ruby, name, "attribute name")?;
    let name = nv.as_bytes();
    edit.with_attributes(|d, n| mutate::remove_attribute(d, n, name))?;
    Ok(rb_self)
}

/// `node.content = text` -> text.
pub fn set_content(ruby: &Ruby, mut this: XmlSelf, text: Value) -> Result<Value, Error> {
    /* A node's type never changes, so it is read before the argument's
     * conversion: a Text, Comment, CDATA or PI node's content is its data,
     * and an Attr's its value - neither changes a child list. */
    let kind = this.read()?.type_(this.id);
    let edit = begin_edit(ruby, &mut this)?;
    let tv = verified_data(ruby, text, "node content")?;
    let bytes = tv.as_bytes();
    let set = |d: &mut XmlDoc, n| mutate::set_content(d, n, bytes);
    /* A Document's or DocumentType's content is nothing: no edit is
     * recorded for it. */
    xml_mut_result(
        match kind.map(|k| dom_rules::text_content(NodeType::from(k))) {
            Some(TextContent::Data) => edit.with_data(set)?,
            Some(TextContent::AttrValue) => edit.with_attributes(set)?,
            Some(TextContent::Nothing) => return Ok(text),
            Some(TextContent::ReplaceChildren) | None => edit.with_arena(set)?,
        },
    )?;
    Ok(text)
}

/* ------------------------------------------------------------------ */
/* building: insertion                                                */
/* ------------------------------------------------------------------ */
/* Every verb is `bridge::xml::insert` at its own `Place`: the checks, the
 * adoption and its completion are the bridge's, as `bridge::html::insert`'s
 * are on the HTML side. */

pub fn add_child(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Place::Child)
}
pub fn before(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Place::Before)
}
pub fn after(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Place::After)
}
pub fn replace(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(ruby, this, arg, Place::Replace)
}

/// `element << node` -> self.
pub fn lshift(ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    insert(ruby, this, arg, Place::Child)?;
    Ok(rb_self)
}

/// `clone_node(deep = false)` -> a detached copy in the same document.
///
/// The ONE method here that takes a node receiver and still uses the factory
/// gate rather than [`Editing`]: it reads the receiver and builds something
/// DETACHED, so there is nothing to invalidate and nothing on the receiver to
/// change - freezing a node does not stop it being copied. Every other method
/// taking `XmlSelf` edits the tree and goes through `begin_edit`.
///
/// A Document is refused: its copy would be a second DOCUMENT node in this
/// same arena, which wraps back to the receiver - so the caller got the
/// original under the name of a copy (and `#clone(freeze: true)` froze it),
/// with a stray node left behind. `XML::Document#dup` is the document copy.
pub fn clone_node(ruby: &Ruby, this: XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let a = crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::positional(0, 1))?;
    let deep = a.optional(0).is_some_and(|v| v.to_bool());
    if crate::bridge::ruby::same_value(this.value, this.document()) {
        return Err(makiri_error("clone_node cannot copy a document; use #dup"));
    }
    let copy = xml_mut_result(with_arena_for_new_node(this.document(), |d| {
        mutate::clone_node(d, this.id, deep)
    })?)?;
    wrap(ruby, copy, this.document())
}

/* ------------------------------------------------------------------ */
/* document factories                                                 */
/* ------------------------------------------------------------------ */

/// `create_element(name, content = nil, attributes = {})` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = crate::bridge::args::scan(
        ruby,
        args,
        crate::bridge::args::Shape {
            rest: true,
            ..crate::bridge::args::Shape::positional(1, 0)
        },
    )?;
    let name = a.required(0);
    let mut content = ruby.qnil().as_value();
    let mut attrs: Option<RHash> = None;
    for &v in a.rest() {
        if let Some(h) = RHash::from_value(v) {
            attrs = Some(h);
        } else if !v.is_nil() {
            content = v;
        }
    }

    let nv = verified_name(ruby, name, "element name")?;
    let cv = optional(content, |v| verified_data(ruby, v, "element content"))?;
    let name = nv.as_bytes();
    let el = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_element(d, name)
    })?)?;
    if let Some(cv) = &cv {
        let text = cv.as_bytes();
        xml_mut_result(with_arena_for_new_node(rb_self, |d| {
            mutate::set_content(d, el, text)
        })?)?;
    }
    let rb_el = wrap(ruby, el, rb_self)?;
    if let Some(h) = attrs {
        /* Each pair goes through the normal validated setter (a key may be
         * a Symbol, a value must be a String), after the pairs are out of
         * the Hash (`hash::each_pair`). */
        /* `rb_el` was wrapped just above, so it converts. */
        let mut el_self = <XmlSelf as crate::bridge::rb::TryConvert>::try_convert(ruby, rb_el)?;
        crate::bridge::hash::each_pair(ruby, h, |k, v| {
            aset_on(ruby, &mut el_self, k, v)?;
            Ok(())
        })?;
    }
    Ok(rb_el)
}

/// `Document#create_element_ns(namespace_uri, qualified_name)` -> Element.
///
/// The DOM's createElementNS, as the HTML Document's is: the name split at
/// its first colon and each half held to the DOM's rule (`ArgumentError`),
/// then the namespace to the name (`Makiri::Error`). The DOM's naming rule,
/// not XML's: `"f}oo"` is an element too, made DOM-loose, which `to_xml`
/// refuses (`mutate::new_dom_element_ns`). Its namespace is decided at once,
/// as the DOM's is, and not re-derived from where it is inserted.
pub fn create_element_ns(
    ruby: &Ruby,
    rb_self: Value,
    ns: Value,
    qname: Value,
) -> Result<Value, Error> {
    let nv = namespace_arg(ruby, ns, "namespace")?;
    let qv = verified_name(ruby, qname, "element qualified name")?;
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    let qname = qv.as_bytes();
    let (prefix, local) =
        dom_extract(ns, qname, dom_name::valid_element_local_name, "DOM element")?;
    /* `verified_name` held the name to `u32`, and both halves are in it:
     * a refusal here is that broken. */
    let len = |n: usize| {
        u32::try_from(n).map_err(|_| crate::bridge::ruby::internal_error("a name past u32"))
    };
    let sp = if prefix.is_empty() {
        Split::unprefixed(len(qname.len())?)
    } else {
        Split::prefixed(len(prefix.len())?, len(local.len())?)
    };
    let el = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_dom_element_ns(d, qname, sp, ns)
    })?)?;
    wrap(ruby, el, rb_self)
}

/// `create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)`
/// -> Element.
///
/// The name is not parsed: the caller gives its split. That is what the DOM's
/// `createElement` needs - its argument is a local name whole, colons and all
/// (`"foo:"`, `"xmlns:foo"`), which neither [`create_element`] (an XML QName)
/// nor [`create_element_ns`] (split at the first colon) can make.
pub fn create_loose_dom_element(
    ruby: &Ruby,
    rb_self: Value,
    qname: Value,
    prefix: Value,
    local: Value,
    ns: Value,
) -> Result<Value, Error> {
    let qv = verified_name(ruby, qname, "qualified name")?;
    let lv = verified_name(ruby, local, "local name")?;
    let pv = optional(prefix, |v| verified_name(ruby, v, "prefix"))?;
    let nv = namespace_arg(ruby, ns, "namespace URI")?;

    let qname = qv.as_bytes();
    let sp = split_loose_dom_name(qname, pv.as_ref().map(|p| p.as_bytes()), lv.as_bytes())
        .map_err(|e| Error::new(crate::bridge::rb::Builtin::ArgumentError, e.message()))?;
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    let el = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_loose_dom_element(d, qname, sp, ns)
    })?)?;
    wrap(ruby, el, rb_self)
}

/// `create_document_type(name, public_id = "", system_id = "")` -> DocumentType.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::positional(1, 2))?;
    let name = a.required(0);
    let nil = ruby.qnil().as_value();
    let nv = verified_name(ruby, name, "doctype name")?;
    let pv = optional(a.optional(0).unwrap_or(nil), |v| {
        verified_text(ruby, v, "doctype public id")
    })?;
    let sv = optional(a.optional(1).unwrap_or(nil), |v| {
        verified_text(ruby, v, "doctype system id")
    })?;
    /* An empty id is absent, like nil, matching the HTML factory and
     * Nokogiri. Built as a `DoctypeId`, never an `Option<&[u8]>` (its doc
     * says why). */
    fn id(v: &Option<crate::bridge::string::RubyText>) -> DoctypeId<'_> {
        match v {
            Some(t) if !t.is_empty() => DoctypeId::written(t.as_bytes()),
            _ => DoctypeId::omitted(),
        }
    }
    let (name, pub_id, sys_id) = (nv.as_bytes(), id(&pv), id(&sv));
    let dt = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_document_type(d, name, pub_id, sys_id)
    })?)?;
    wrap(ruby, dt, rb_self)
}

/// The shared body of the leaf-data factories.
fn create_chardata(
    ruby: &Ruby,
    rb_self: Value,
    text: Value,
    type_: ArenaKind,
    what: &str,
) -> Result<Value, Error> {
    let tv = verified_data(ruby, text, what)?;
    let bytes = tv.as_bytes();
    let n = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_chardata(d, type_, bytes)
    })?)?;
    wrap(ruby, n, rb_self)
}

pub fn create_text_node(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, ArenaKind::Text, "text content")
}
pub fn create_comment(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, ArenaKind::Comment, "comment content")
}
pub fn create_cdata(ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(ruby, rb_self, t, ArenaKind::CDataSection, "CDATA content")
}

pub fn create_pi(ruby: &Ruby, rb_self: Value, target: Value, data: Value) -> Result<Value, Error> {
    let tg = verified_name(ruby, target, "PI target")?;
    let dt = verified_data(ruby, data, "PI data")?;
    let (target, data) = (tg.as_bytes(), dt.as_bytes());
    let pi = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_pi(d, target, data)
    })?)?;
    wrap(ruby, pi, rb_self)
}

/// `Document#import_node(node, deep = false)` - the DOM's importNode.
pub fn import_node(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::positional(1, 1))?;
    let deep = a.optional(0).is_some_and(|v| v.to_bool());
    wrap(
        ruby,
        import_copy(ruby, rb_self, a.required(0), deep)?,
        rb_self,
    )
}
