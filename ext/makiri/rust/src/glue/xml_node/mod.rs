//! The XML node: wrapping and unwrapping an arena node, and every method that
//! answers from the tree without writing to it - name and namespace, the DTD
//! identifiers, namespace introspection, content, navigation and attributes.
//!
//! The writing half is in the submodules: `mutate` (mutation and the document
//! factories) and `serialize` (`#to_xml`, `#canonicalize`).
//!
//! Nothing here touches Lexbor. The node layout comes from `crate::xml::model`,
//! the XML engine's own declaration, so no offset or type constant is restated.

#![allow(clippy::missing_safety_doc)]

pub mod abi;
pub mod mutate;
pub mod ns;
pub mod read;
pub mod serialize;

use core::ffi::c_void;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, RClass, Ruby, Value};
use rb_sys::VALUE;

use self::abi::*;
use super::abi::{doc_parsed, parsed_xml_doc, NodeData, CLASS_DOCUMENT, CLASS_NODE_SET};

/// Wrap an arena node into its `Makiri::XML::*` leaf.
///
/// NULL becomes nil, and the DOCUMENT node maps back onto the Ruby Document
/// rather than getting a second wrapper - so `node.document.equal?(doc)` holds
/// and the arena has exactly one owner.
///
/// The signature is the representation-opaque one every caller shares (see
/// `glue::abi`); the cast to the XML node is justified by this being the XML
/// wrap path.
pub unsafe extern "C" fn wrap_xml_node(node: *mut c_void, document: VALUE) -> VALUE {
    let id = NodeId::from_token(node as usize);
    if id.is_invalid() {
        return rb_sys::Qnil as VALUE;
    }
    let xdoc = doc_of(document);
    let ty = (*xdoc).type_(id);
    if ty == Some(NodeType::Document) {
        return document;
    }
    let klass = match ty {
        Some(NodeType::Element) => CLASS_XML_ELEMENT.raw(),
        Some(NodeType::Attribute) => CLASS_XML_ATTR.raw(),
        Some(NodeType::Text) => CLASS_XML_TEXT.raw(),
        Some(NodeType::CData) => CLASS_XML_CDATA_SECTION.raw(),
        Some(NodeType::Comment) => CLASS_XML_COMMENT.raw(),
        Some(NodeType::Pi) => CLASS_XML_PROCESSING_INSTRUCTION.raw(),
        Some(NodeType::Doctype) => CLASS_XML_DOCUMENT_TYPE.raw(),
        Some(NodeType::Fragment) => CLASS_XML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_XML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `wrap_zeroed`. */
    crate::bridge::ruby::wrap_zeroed::<NodeData>(
        klass,
        XML_NODE_TYPE.as_ptr(),
        |nd| nd.node = node,
        |nd| nd.document = document,
    )
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node -
/// the representation check is Ruby's own type machinery, not a flag we could
/// forget to test.
pub unsafe fn xml_node_unwrap(rb_self: VALUE) -> Result<*mut c_void, magnus::Error> {
    let v = Value::from_raw(rb_self);
    if is_a(v, &CLASS_XML_DOCUMENT) {
        let xdoc = parsed_xml_doc(doc_parsed(rb_self)?) as *mut XmlDoc;
        return Ok((*xdoc).doc_node().to_token() as *mut c_void);
    }
    let nd =
        crate::bridge::ruby::typed_data(Value::from_raw(rb_self), &XML_NODE_TYPE)? as *mut NodeData;
    Ok((*nd).node)
}

/// The XML document behind a Document or node wrapper (`Document` VALUE).
///
/// `document` must be a Document VALUE the caller has established - a node's
/// keepalive Document or an XML Document receiver.
pub unsafe fn doc_of(document: VALUE) -> *mut XmlDoc {
    parsed_xml_doc(crate::glue::doc::doc_parsed_known(document)) as *mut XmlDoc
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node at
/// the type boundary, like [`xml_node_unwrap`].
pub unsafe fn xml_node_document(rb_self: VALUE) -> Result<VALUE, magnus::Error> {
    let v = Value::from_raw(rb_self);
    if is_a(v, &CLASS_XML_DOCUMENT) {
        return Ok(rb_self);
    }
    let nd =
        crate::bridge::ruby::typed_data(Value::from_raw(rb_self), &XML_NODE_TYPE)? as *mut NodeData;
    Ok((*nd).document)
}

/// Wrap a node reached from a checked receiver, under its Document.
pub unsafe fn xml_wrap_rel_value(this: XmlSelf, rel: NodeId) -> Value {
    wrap(rel, this.document)
}

/* ---- the Rust-side conveniences the submodules use ---- */

/// [`xml_node_unwrap`] with the node id typed.
pub unsafe fn unwrap(v: Value) -> Result<NodeId, magnus::Error> {
    Ok(NodeId::from_token(xml_node_unwrap(v.as_raw())? as usize))
}

/// A method receiver already checked to be an XML node or XML Document; see
/// the HTML twin, `html_node::HtmlSelf`.
#[derive(Clone, Copy)]
pub struct XmlSelf {
    pub value: Value,
    pub id: NodeId,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl magnus::TryConvert for XmlSelf {
    fn try_convert(value: Value) -> Result<Self, magnus::Error> {
        // SAFETY: magnus converts the receiver under the GVL.
        unsafe {
            let id = unwrap(value)?;
            let document = Value::from_raw(xml_node_document(value.as_raw())?);
            Ok(XmlSelf {
                value,
                id,
                document,
            })
        }
    }
}

impl XmlSelf {
    /// The arena behind the receiver's Document.
    pub unsafe fn doc(self) -> *mut XmlDoc {
        doc_of(self.document.as_raw())
    }
}

/// The keepalive Document of an XML node. `Err(TypeError)` for an HTML node.
pub unsafe fn node_document(v: Value) -> Result<Value, magnus::Error> {
    Ok(Value::from_raw(xml_node_document(v.as_raw())?))
}

/// The XML document behind a node wrapper. `Err(TypeError)` for an HTML node.
pub unsafe fn doc(v: Value) -> Result<*mut XmlDoc, magnus::Error> {
    Ok(doc_of(xml_node_document(v.as_raw())?))
}

pub unsafe fn wrap(node: NodeId, document: Value) -> Value {
    Value::from_raw(wrap_xml_node(
        node.to_token() as *mut c_void,
        document.as_raw(),
    ))
}

pub use crate::glue::node::node_equals;
pub use crate::glue::node::node_hash;
pub use crate::glue::node::node_pointer_id;

/// The shape `rb_define_method` wants. Ruby dispatches on the declared arity, so
/// a 0- and a 1-argument method are both reached through this one type.
type RbMethod = unsafe extern "C" fn() -> VALUE;

/// Bind a method implemented by a C-ABI function, for the identity trio above.
///
/// They are bound as function pointers rather than re-implemented, because
/// identity depends only on the node pointer and so must be the SAME code the
/// HTML side runs - two implementations would be two answers.
unsafe fn define_c_method(module: VALUE, name: &core::ffi::CStr, f: RbMethod, arity: i32) {
    rb_sys::rb_define_method(module, name.as_ptr(), Some(f), arity);
}

/// `init_xml_node_read` - the same entry point `init_xml_node` calls.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn init_xml_node_read() {
    let ruby = Ruby::get_unchecked();
    let m = magnus::RModule::from_value(MOD_XML_NODE_METHODS.value())
        .expect("Makiri::XML::NodeMethods");

    m.define_method("name", method!(read::name, 0))
        .expect("#name");
    m.define_method("local_name", method!(read::local_name, 0))
        .expect("#local_name");
    m.define_method("prefix", method!(read::prefix, 0))
        .expect("#prefix");
    m.define_method("namespace_uri", method!(read::namespace_uri, 0))
        .expect("#namespace_uri");
    m.define_method("node_type", method!(read::node_type, 0))
        .expect("#node_type");

    /* Namespace introspection, plus the (prefix, href) value object it hands back. */
    let m_xml = magnus::RModule::from_value(MOD_XML.value()).expect("Makiri::XML");
    let ns_class: RClass = m_xml
        .define_class("Namespace", ruby.class_object())
        .expect("Makiri::XML::Namespace");
    ns::set_namespace_class(ns_class);
    ns_class
        .define_method("prefix", method!(ns::ns_prefix, 0))
        .expect("Namespace#prefix");
    ns_class
        .define_method("href", method!(ns::ns_href, 0))
        .expect("Namespace#href");
    ns_class
        .define_method("to_s", method!(ns::ns_href, 0))
        .expect("Namespace#to_s");
    ns_class
        .define_method("==", method!(ns::ns_equal, 1))
        .expect("Namespace#==");
    ns_class
        .define_method("eql?", method!(ns::ns_equal, 1))
        .expect("Namespace#eql?");
    ns_class
        .define_method("hash", method!(ns::ns_hash, 0))
        .expect("Namespace#hash");
    ns_class
        .define_method("inspect", method!(ns::ns_inspect, 0))
        .expect("Namespace#inspect");

    m.define_method("namespace", method!(ns::namespace, 0))
        .expect("#namespace");
    m.define_method(
        "namespace_definitions",
        method!(ns::namespace_definitions, 0),
    )
    .expect("#namespace_definitions");
    m.define_method("namespaces", method!(ns::namespaces, 0))
        .expect("#namespaces");
    m.define_method("collect_namespaces", method!(ns::collect_namespaces, 0))
        .expect("#collect_namespaces");

    for name in ["content", "text", "inner_text"] {
        m.define_method(name, method!(read::content, 0))
            .expect("#content");
    }
    m.define_method("value", method!(read::value, 0))
        .expect("#value");

    m.define_method("document", method!(read::get_document, 0))
        .expect("#document");
    m.define_method("parent", method!(read::parent, 0))
        .expect("#parent");
    for name in ["next", "next_sibling"] {
        m.define_method(name, method!(read::next, 0))
            .expect("#next");
    }
    for name in ["previous", "previous_sibling"] {
        m.define_method(name, method!(read::previous, 0))
            .expect("#previous");
    }
    m.define_method("child", method!(read::first_child, 0))
        .expect("#child");
    m.define_method("last_element_child", method!(read::last_child, 0))
        .expect("#last_element_child");
    m.define_method("children", method!(read::children, 0))
        .expect("#children");
    m.define_method("element_children", method!(read::element_children, 0))
        .expect("#element_children");

    m.define_method("[]", method!(read::aref, 1)).expect("#[]");
    m.define_method("attribute_nodes", method!(read::attribute_nodes, 0))
        .expect("#attribute_nodes");
    m.define_method(
        "attribute_by_qualified_name",
        method!(read::attribute_by_qualified_name, 1),
    )
    .expect("#attribute_by_qualified_name");
    m.define_method(
        "attribute_value_by_qualified_name",
        method!(read::attribute_value_by_qualified_name, 1),
    )
    .expect("#attribute_value_by_qualified_name");

    /* Node identity by the underlying pointer, so #path, NodeSet dedup, Set and
     * Hash all work - the same contract HTML nodes have, from the same code. */
    let equals: RbMethod =
        core::mem::transmute(node_equals as unsafe extern "C" fn(VALUE, VALUE) -> VALUE);
    let hash: RbMethod = core::mem::transmute(node_hash as unsafe extern "C" fn(VALUE) -> VALUE);
    let ptr_id: RbMethod =
        core::mem::transmute(node_pointer_id as unsafe extern "C" fn(VALUE) -> VALUE);
    define_c_method(MOD_XML_NODE_METHODS.raw(), c"==", equals, 1);
    define_c_method(MOD_XML_NODE_METHODS.raw(), c"eql?", equals, 1);
    define_c_method(MOD_XML_NODE_METHODS.raw(), c"hash", hash, 0);
    define_c_method(MOD_XML_NODE_METHODS.raw(), c"pointer_id", ptr_id, 0);

    /* DocumentType identifiers; #public_id is the Nokogiri-style alias of
     * #external_id, and #name comes from the shared reader above. */
    let dt =
        RClass::from_value(CLASS_XML_DOCUMENT_TYPE.value()).expect("Makiri::XML::DocumentType");
    for name in ["external_id", "public_id"] {
        dt.define_method(name, method!(read::dtd_external_id, 0))
            .expect("#external_id");
    }
    dt.define_method("system_id", method!(read::dtd_system_id, 0))
        .expect("#system_id");

    let _ = (CLASS_DOCUMENT.raw(), CLASS_NODE_SET.raw());
}

/// `init_xml_node` - the whole XML node surface, once the mutation half is
/// Rust too. Until then `glue/ruby_xml_node.c` provides it and calls the reader
/// half's entry point above.
///
/// # Safety
/// From `Init_makiri`.
pub unsafe extern "C" fn init_xml_node() {
    /* Serialization: #to_xml / #canonicalize, and the refused HTML ones. */
    init_xml_node_serialize();
    init_xml_node_read();

    let m = magnus::RModule::from_value(MOD_XML_NODE_METHODS.value())
        .expect("Makiri::XML::NodeMethods");
    let doc = RClass::from_value(CLASS_XML_DOCUMENT.value()).expect("XML::Document");

    /* In-place edits. Detach-never-destroy; the primitives live in
     * xml/mkr_xml_mutate.c. */
    for name in ["remove", "unlink"] {
        m.define_method(name, method!(mutate::remove, 0))
            .expect("#remove");
    }
    m.define_method("[]=", method!(mutate::aset, 2))
        .expect("#[]=");
    for name in ["delete", "remove_attribute"] {
        m.define_method(name, method!(mutate::delete, 1))
            .expect("#delete");
    }
    m.define_method("set_attribute_ns", method!(mutate::set_attribute_ns, 3))
        .expect("#set_attribute_ns");
    m.define_method(
        "remove_attribute_ns",
        method!(mutate::remove_attribute_ns, 2),
    )
    .expect("#remove_attribute_ns");
    m.define_method("content=", method!(mutate::set_content, 1))
        .expect("#content=");
    m.define_method("name=", method!(mutate::set_name, 1))
        .expect("#name=");

    /* Building. Insertion accepts a single Makiri::XML node; one from another
     * document is deep-copied into this one. */
    m.define_method("add_child", method!(mutate::add_child, 1))
        .expect("#add_child");
    m.define_method("<<", method!(mutate::lshift, 1))
        .expect("#<<");
    for name in ["add_previous_sibling", "before"] {
        m.define_method(name, method!(mutate::before, 1))
            .expect("#before");
    }
    for name in ["add_next_sibling", "after"] {
        m.define_method(name, method!(mutate::after, 1))
            .expect("#after");
    }
    m.define_method("replace", method!(mutate::replace, 1))
        .expect("#replace");

    /* Document factories. The node-class .new constructors and Document#root=
     * are pure delegations to these, defined once in the Ruby layer. */
    doc.define_method("create_element", method!(mutate::create_element, -1))
        .expect("#create_element");
    doc.define_method(
        "create_loose_dom_element",
        method!(mutate::create_loose_dom_element, 4),
    )
    .expect("#create_loose_dom_element");
    doc.define_method(
        "create_document_type",
        method!(mutate::create_document_type, -1),
    )
    .expect("#create_document_type");
    doc.define_method("create_text_node", method!(mutate::create_text_node, 1))
        .expect("#create_text_node");
    doc.define_method("create_comment", method!(mutate::create_comment, 1))
        .expect("#create_comment");
    for name in ["create_cdata", "create_cdata_node"] {
        doc.define_method(name, method!(mutate::create_cdata, 1))
            .expect("#create_cdata");
    }
    doc.define_method(
        "create_processing_instruction",
        method!(mutate::create_pi, 2),
    )
    .expect("#create_processing_instruction");
    doc.define_method("import_node", method!(mutate::import_node, -1))
        .expect("#import_node");

    m.define_method("clone_node", method!(mutate::clone_node, -1))
        .expect("#clone_node");
}

use self::serialize::init_xml_node_serialize;
