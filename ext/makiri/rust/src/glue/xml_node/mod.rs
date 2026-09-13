//! The XML node's reading half (glue/ruby_xml_node_read.c).
//!
//! Wrapping and unwrapping an arena node, and every method that answers from the
//! tree without writing to it: name and namespace, the DTD identifiers,
//! namespace introspection, content, navigation and attributes.
//!
//! The writing half - serialization, canonicalization, mutation and the document
//! factories - is still C (`glue/ruby_xml_node.c`). The boundary between them is
//! one-directional: nothing here calls into it, and it reaches back for only two
//! functions, [`mkr_xml_node_document`] and [`mkr_xml_wrap_rel`], which is why
//! those two are exported under their C names.
//!
//! Nothing here touches Lexbor. The node layout comes from `crate::xml::abi`,
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
use super::abi::{mkr_cDocument, mkr_cNodeSet, mkr_doc_parsed, mkr_parsed_xml_doc, NodeData};

/// Wrap an arena node into its `Makiri::XML::*` leaf.
///
/// NULL becomes nil, and the DOCUMENT node maps back onto the Ruby Document
/// rather than getting a second wrapper - so `node.document.equal?(doc)` holds
/// and the arena has exactly one owner.
///
/// The signature is the representation-opaque one every caller shares (see
/// `glue::abi`); the cast to the XML node is justified by this being the XML
/// wrap path.
pub unsafe extern "C" fn mkr_wrap_xml_node(node: *mut c_void, document: VALUE) -> VALUE {
    let id = NodeId::from_token(node as usize);
    if id.is_invalid() {
        return rb_sys::Qnil as VALUE;
    }
    let xdoc = mkr_doc_of(document);
    let ty = (*xdoc).type_(id);
    if ty == T_DOCUMENT {
        return document;
    }
    let klass = match ty {
        T_ELEMENT => mkr_cXmlElement,
        T_ATTRIBUTE => mkr_cXmlAttr,
        T_TEXT => mkr_cXmlText,
        T_CDATA => mkr_cXmlCDATASection,
        T_COMMENT => mkr_cXmlComment,
        T_PI => mkr_cXmlProcessingInstruction,
        T_DOCTYPE => mkr_cXmlDocumentType,
        T_FRAGMENT => mkr_cXmlDocumentFragment,
        _ => mkr_cXmlNode,
    };

    /* Fill the struct BEFORE handing it to Ruby: once wrapped, the object is
     * reachable and a GC would run the type's mark over whatever is there. */
    let nd =
        rb_sys::ruby_xmalloc(core::mem::size_of::<NodeData>() as rb_sys::size_t) as *mut NodeData;
    (*nd).node = node;
    (*nd).document = document;
    rb_sys::rb_data_typed_object_wrap(klass, nd as *mut c_void, mkr_xml_node_type.as_ptr())
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which **raises** TypeError for an HTML node -
/// the representation check is Ruby's own type machinery, not a flag we could
/// forget to test.
pub unsafe extern "C" fn mkr_xml_node_unwrap(rb_self: VALUE) -> *mut c_void {
    let v = Value::from_raw(rb_self);
    if is_a(v, mkr_cXmlDocument) {
        let xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(rb_self)) as *mut XmlDoc;
        return (*xdoc).doc_node().to_token() as *mut c_void;
    }
    let nd = rb_sys::rb_check_typeddata(rb_self, mkr_xml_node_type.as_ptr()) as *mut NodeData;
    (*nd).node
}

/// The XML document behind a Document or node wrapper (`Document` VALUE).
pub unsafe fn mkr_doc_of(document: VALUE) -> *mut XmlDoc {
    mkr_parsed_xml_doc(mkr_doc_parsed(document)) as *mut XmlDoc
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node at
/// the type boundary, like [`mkr_xml_node_unwrap`].
pub unsafe extern "C" fn mkr_xml_node_document(rb_self: VALUE) -> VALUE {
    let v = Value::from_raw(rb_self);
    if is_a(v, mkr_cXmlDocument) {
        return rb_self;
    }
    let nd = rb_sys::rb_check_typeddata(rb_self, mkr_xml_node_type.as_ptr()) as *mut NodeData;
    (*nd).document
}

/// Wrap a node reached from `rb_self`, under `rb_self`'s Document. One of the
/// two functions the still-C serialization half calls.
pub unsafe fn mkr_xml_wrap_rel(rb_self: VALUE, rel: NodeId) -> VALUE {
    mkr_wrap_xml_node(
        rel.to_token() as *mut c_void,
        mkr_xml_node_document(rb_self),
    )
}

/// The same, in Rust terms.
pub unsafe fn mkr_xml_wrap_rel_value(rb_self: Value, rel: NodeId) -> Value {
    Value::from_raw(mkr_xml_wrap_rel(rb_self.as_raw(), rel))
}

/* ---- the Rust-side conveniences the submodules use ---- */

/// [`mkr_xml_node_unwrap`] with the node id typed.
pub unsafe fn unwrap(rb_self: Value) -> NodeId {
    NodeId::from_token(mkr_xml_node_unwrap(rb_self.as_raw()) as usize)
}

pub unsafe fn node_document(rb_self: Value) -> Value {
    Value::from_raw(mkr_xml_node_document(rb_self.as_raw()))
}

/// The XML document behind `rb_self`'s wrapper.
pub unsafe fn doc(rb_self: Value) -> *mut XmlDoc {
    mkr_doc_of(mkr_xml_node_document(rb_self.as_raw()))
}

pub unsafe fn wrap(node: NodeId, document: Value) -> Value {
    Value::from_raw(mkr_wrap_xml_node(
        node.to_token() as *mut c_void,
        document.as_raw(),
    ))
}

pub use crate::glue::node::mkr_node_equals;
pub use crate::glue::node::mkr_node_hash;
pub use crate::glue::node::mkr_node_pointer_id;

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

/// `mkr_init_xml_node_read` - the same entry point `mkr_init_xml_node` calls.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn mkr_init_xml_node_read() {
    let ruby = Ruby::get_unchecked();
    let m = magnus::RModule::from_value(Value::from_raw(mkr_mXmlNodeMethods))
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
    let m_xml = magnus::RModule::from_value(Value::from_raw(mkr_mXML)).expect("Makiri::XML");
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
        core::mem::transmute(mkr_node_equals as unsafe extern "C" fn(VALUE, VALUE) -> VALUE);
    let hash: RbMethod =
        core::mem::transmute(mkr_node_hash as unsafe extern "C" fn(VALUE) -> VALUE);
    let ptr_id: RbMethod =
        core::mem::transmute(mkr_node_pointer_id as unsafe extern "C" fn(VALUE) -> VALUE);
    define_c_method(mkr_mXmlNodeMethods, c"==", equals, 1);
    define_c_method(mkr_mXmlNodeMethods, c"eql?", equals, 1);
    define_c_method(mkr_mXmlNodeMethods, c"hash", hash, 0);
    define_c_method(mkr_mXmlNodeMethods, c"pointer_id", ptr_id, 0);

    /* DocumentType identifiers; #public_id is the Nokogiri-style alias of
     * #external_id, and #name comes from the shared reader above. */
    let dt = RClass::from_value(Value::from_raw(mkr_cXmlDocumentType))
        .expect("Makiri::XML::DocumentType");
    for name in ["external_id", "public_id"] {
        dt.define_method(name, method!(read::dtd_external_id, 0))
            .expect("#external_id");
    }
    dt.define_method("system_id", method!(read::dtd_system_id, 0))
        .expect("#system_id");

    let _ = (mkr_cDocument, mkr_cNodeSet);
}

/// `mkr_init_xml_node` - the whole XML node surface, once the mutation half is
/// Rust too. Until then `glue/ruby_xml_node.c` provides it and calls the reader
/// half's entry point above.
///
/// # Safety
/// From `Init_makiri`.
pub unsafe extern "C" fn mkr_init_xml_node() {
    /* Serialization (#to_xml / #canonicalize, and the refused HTML ones) is
     * still C: ruby_xml_node_serialize.c. */
    mkr_init_xml_node_serialize();
    mkr_init_xml_node_read();

    let m = magnus::RModule::from_value(Value::from_raw(mkr_mXmlNodeMethods))
        .expect("Makiri::XML::NodeMethods");
    let doc = RClass::from_value(Value::from_raw(mkr_cXmlDocument)).expect("XML::Document");

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

use self::serialize::mkr_init_xml_node_serialize;
