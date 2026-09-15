//! The HTML (Lexbor) node representation (glue/ruby_html_node.c).
//!
//! Wrapping an `lxb_dom_node_t` into a `Makiri::HTML::*` leaf, the HTML
//! node-pointer accessor, and the reader methods that hang off
//! `Makiri::HTML::NodeMethods`. The XML counterpart is `glue::xml_node`; the
//! representation-neutral node core - the `rb_data_type_t` chain and the
//! kind-agnostic accessors - is `glue::node`.
//!
//! # Two symbols here are load-bearing for C
//!
//! [`mkr_wrap_html_node`] and [`mkr_html_node_unwrap`] are called by four and
//! eight other translation units, several still C, so their signatures are
//! fixed. `glue::abi`'s `agree` module checks the definitions here against the
//! declarations there.
//!
//! # Nothing here is declared twice
//!
//! Every Lexbor accessor comes from `glue::abi`, which re-exports the generated
//! bindings and the hand-declared `_noi` twins. Allowlisting a name in build.rs
//! and finding no binding is what identifies an `lxb_inline` function; eight of
//! the eighteen readers this file needs turned out to be inline-only, and on
//! macOS a hand-written declaration of one of those links to nothing and becomes
//! a NULL call at run time rather than a link error.

#![allow(clippy::missing_safety_doc)]

pub mod read;

pub mod mutate;

use core::ffi::c_void;

use magnus::rb_sys::FromRawValue;
use magnus::{method, prelude::*, RClass, Ruby, Value};
use rb_sys::VALUE;

use super::abi::{
    html_node_methods, is_kind_of, mkr_cDocument, mkr_cXmlDocument, mkr_html_doc_unwrap, LxbNode,
    NodeData,
};
/* Only the mutation half registers on the Document class. */
use super::abi::mkr_cHtmlDocument;

/* ------------------------------------------------------------------ *
 * the DOM node types                                                 *
 * ------------------------------------------------------------------ */

/// Generated, never transcribed - the reason is in `build.rs`.
pub mod ty {
    use crate::lexbor_abi as lxb;
    pub const ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
    pub const ATTRIBUTE: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ATTRIBUTE;
    pub const TEXT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT;
    pub const CDATA: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION;
    pub const PI: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION;
    pub const COMMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT;
    pub const DOCUMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT;
    pub const DOCTYPE: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;
    pub const FRAGMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
}

pub use crate::glue::doc::mkr_node_clone_node;
pub use crate::glue::node::mkr_html_node_type;
pub use crate::glue::node::mkr_node_equals;
pub use crate::glue::node::mkr_node_hash;
pub use crate::glue::node::mkr_node_pointer_id;
pub use crate::init::mkr_cHtmlAttr;
pub use crate::init::mkr_cHtmlCDATASection;
pub use crate::init::mkr_cHtmlComment;
pub use crate::init::mkr_cHtmlDocumentFragment;
pub use crate::init::mkr_cHtmlDocumentType;
pub use crate::init::mkr_cHtmlElement;
pub use crate::init::mkr_cHtmlNode;
pub use crate::init::mkr_cHtmlProcessingInstruction;
pub use crate::init::mkr_cHtmlText;

/* ------------------------------------------------------------------ *
 * wrap / unwrap                                                      *
 * ------------------------------------------------------------------ */

/// Wrap an `lxb_dom_node_t` into its `Makiri::HTML::*` leaf.
///
/// NULL becomes nil, and the DOCUMENT node maps back onto the Ruby Document
/// rather than getting a second wrapper. A DOM node type with no specific leaf
/// (entity/notation - Lexbor's HTML parser does not produce these) falls back to
/// the generic `Makiri::HTML::Node` rather than being misclassified as an
/// Element.
pub unsafe extern "C" fn mkr_wrap_html_node(node: *mut LxbNode, document: VALUE) -> VALUE {
    if node.is_null() {
        return rb_sys::Qnil as VALUE;
    }
    if (*node).type_ == ty::DOCUMENT {
        return document;
    }

    let klass = match (*node).type_ {
        ty::ELEMENT => mkr_cHtmlElement,
        ty::ATTRIBUTE => mkr_cHtmlAttr,
        ty::TEXT => mkr_cHtmlText,
        ty::COMMENT => mkr_cHtmlComment,
        ty::CDATA => mkr_cHtmlCDATASection,
        ty::PI => mkr_cHtmlProcessingInstruction,
        ty::DOCTYPE => mkr_cHtmlDocumentType,
        ty::FRAGMENT => mkr_cHtmlDocumentFragment,
        _ => mkr_cHtmlNode,
    };

    /* Allocate zeroed, wrap, and only then store the Document. The wrap
     * allocates, so it is a GC point, and a VALUE already sitting in this
     * malloc'd struct is seen by no mark there: compaction can move it out from
     * under the stored copy. Zeroed, the field reads as `false` to the mark
     * until it is set, and `document` - used after the wrap - stays on the
     * machine stack across it, where the conservative scan pins it. */
    let nd = rb_sys::ruby_xcalloc(1, core::mem::size_of::<NodeData>() as rb_sys::size_t)
        as *mut NodeData;
    (*nd).node = node as *mut c_void;
    let obj =
        rb_sys::rb_data_typed_object_wrap(klass, nd as *mut c_void, mkr_html_node_type.as_ptr());
    (*nd).document = document;
    obj
}

/// The `lxb_dom_node_t` behind an HTML node or HTML Document.
///
/// **Raises** TypeError for an XML node or Document: the typed-data check is
/// against `mkr_html_node_type`, which an XML node - wrapped under
/// `mkr_xml_node_type` - does not satisfy. Every HTML-glue site that
/// dereferences a node or hands its pointer to Lexbor goes through here, for
/// `self` and arguments alike.
pub unsafe extern "C" fn mkr_html_node_unwrap(rb_node: VALUE) -> *mut LxbNode {
    if is_kind_of(Value::from_raw(rb_node), mkr_cDocument) {
        if is_kind_of(Value::from_raw(rb_node), mkr_cXmlDocument) {
            rb_sys::rb_raise(
                rb_sys::rb_eTypeError,
                c"expected an HTML node, got a Makiri::XML::Document".as_ptr(),
            );
        }
        return mkr_html_doc_unwrap(rb_node) as *mut LxbNode;
    }
    let nd = rb_sys::rb_check_typeddata(rb_node, mkr_html_node_type.as_ptr()) as *mut NodeData;
    (*nd).node as *mut LxbNode
}

/* ---- the Rust-side conveniences the reader module uses ---- */

/// [`mkr_html_node_unwrap`] in Rust terms.
///
/// It raises, so it is called where nothing needs dropping - which in these
/// readers means first, before any Ruby object or buffer exists.
pub unsafe fn unwrap(rb_self: Value) -> *mut LxbNode {
    use magnus::rb_sys::AsRawValue;
    mkr_html_node_unwrap(rb_self.as_raw())
}

pub unsafe fn wrap(node: *mut LxbNode, document: Value) -> Value {
    use magnus::rb_sys::AsRawValue;
    Value::from_raw(mkr_wrap_html_node(node, document.as_raw()))
}

/// The keepalive Document of a node, from the kind-agnostic accessor.
pub unsafe fn node_document(rb_self: Value) -> Value {
    use magnus::rb_sys::AsRawValue;
    Value::from_raw(super::abi::mkr_node_document(rb_self.as_raw()))
}

/* ------------------------------------------------------------------ *
 * registration                                                       *
 * ------------------------------------------------------------------ */

/// The shape `rb_define_method` wants. Ruby dispatches on the declared arity,
/// so every arity is reached through this one type.
type RbMethod = unsafe extern "C" fn() -> VALUE;

/// Bind a method implemented by a C-ABI function, for the four that must stay
/// shared with the XML side or live in the still-C mutation half.
unsafe fn define_c_method(module: VALUE, name: &core::ffi::CStr, f: RbMethod, arity: i32) {
    rb_sys::rb_define_method(module, name.as_ptr(), Some(f), arity);
}

/// `mkr_init_node` - the HTML node surface.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn mkr_init_node() {
    let _ = Ruby::get_unchecked();
    let m = html_node_methods();

    m.define_method("name", method!(read::name, 0))
        .expect("#name");
    m.define_method("namespace_uri", method!(read::namespace_uri, 0))
        .expect("#namespace_uri");
    m.define_method("prefix", method!(read::prefix, 0))
        .expect("#prefix");
    m.define_method("local_name", method!(read::local_name, 0))
        .expect("#local_name");
    m.define_method("tag_name", method!(read::tag_name, 0))
        .expect("#tag_name");
    m.define_method("target", method!(read::pi_target, 0))
        .expect("#target");
    m.define_method("node_type", method!(read::node_type, 0))
        .expect("#node_type");
    for name in ["content", "text", "inner_text"] {
        m.define_method(name, method!(read::content, 0))
            .expect("#content");
    }

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
    m.define_method("next_element", method!(read::next_element, 0))
        .expect("#next_element");
    m.define_method("previous_element", method!(read::previous_element, 0))
        .expect("#previous_element");

    m.define_method("child", method!(read::child, 0))
        .expect("#child");
    m.define_method("children", method!(read::children, 0))
        .expect("#children");
    for name in ["element_children", "elements"] {
        m.define_method(name, method!(read::element_children, 0))
            .expect("#element_children");
    }
    m.define_method("first_element_child", method!(read::first_element_child, 0))
        .expect("#first_element_child");
    m.define_method("last_element_child", method!(read::last_element_child, 0))
        .expect("#last_element_child");
    m.define_method("ancestors", method!(read::ancestors, 0))
        .expect("#ancestors");

    m.define_method("[]", method!(read::aref, 1)).expect("#[]");
    m.define_method("key?", method!(read::has_key, 1))
        .expect("#key?");
    m.define_method("keys", method!(read::keys, 0))
        .expect("#keys");
    m.define_method("values", method!(read::values, 0))
        .expect("#values");
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
    m.define_method("value", method!(read::value, 0))
        .expect("#value");
    m.define_method("line", method!(read::line, 0))
        .expect("#line");

    /* Identity is by the node pointer and shared with the XML side; document
     * order is HTML-only and lives in read.rs. */
    let methods = m.as_raw();
    let equals: RbMethod =
        core::mem::transmute(mkr_node_equals as unsafe extern "C" fn(VALUE, VALUE) -> VALUE);
    let hash: RbMethod =
        core::mem::transmute(mkr_node_hash as unsafe extern "C" fn(VALUE) -> VALUE);
    let ptr_id: RbMethod =
        core::mem::transmute(mkr_node_pointer_id as unsafe extern "C" fn(VALUE) -> VALUE);
    let clone: RbMethod = core::mem::transmute(
        mkr_node_clone_node as unsafe extern "C" fn(core::ffi::c_int, *const VALUE, VALUE) -> VALUE,
    );
    define_c_method(methods, c"==", equals, 1);
    define_c_method(methods, c"eql?", equals, 1);
    define_c_method(methods, c"hash", hash, 0);
    define_c_method(methods, c"pointer_id", ptr_id, 0);
    define_c_method(methods, c"clone_node", clone, -1);

    m.define_method("<=>", method!(read::spaceship, 1))
        .expect("#<=>");

    /* DocumentType identifiers (WHATWG DOM names; external_id is the
     * Nokogiri-compatible alias for public_id). */
    let dt = RClass::from_value(Value::from_raw(mkr_cHtmlDocumentType))
        .expect("Makiri::HTML::DocumentType");
    for name in ["public_id", "external_id"] {
        dt.define_method(name, method!(read::doctype_public_id, 0))
            .expect("#public_id");
    }
    dt.define_method("system_id", method!(read::doctype_system_id, 0))
        .expect("#system_id");

    /* <template> contents (WHATWG DOM HTMLTemplateElement.content). */
    let el = RClass::from_value(Value::from_raw(mkr_cHtmlElement)).expect("Makiri::HTML::Element");
    el.define_method("content_fragment", method!(read::content_fragment, 0))
        .expect("#content_fragment");
}

use magnus::rb_sys::AsRawValue;

/// `mkr_init_mutate` - the HTML node's mutators and the Document factories.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn mkr_init_mutate() {
    let m = html_node_methods();
    let doc = RClass::from_value(Value::from_raw(mkr_cHtmlDocument)).expect("HTML::Document");

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
    for name in ["remove", "unlink"] {
        m.define_method(name, method!(mutate::remove, 0))
            .expect("#remove");
    }
    m.define_method("replace", method!(mutate::replace, 1))
        .expect("#replace");

    m.define_method("inner_html=", method!(mutate::set_inner_html, 1))
        .expect("#inner_html=");
    m.define_method("outer_html=", method!(mutate::set_outer_html, 1))
        .expect("#outer_html=");

    m.define_method("[]=", method!(mutate::aset, 2))
        .expect("#[]=");
    m.define_method("set_attribute_ns", method!(mutate::set_attribute_ns, 3))
        .expect("#set_attribute_ns");
    m.define_method(
        "remove_attribute_ns",
        method!(mutate::remove_attribute_ns, 2),
    )
    .expect("#remove_attribute_ns");
    for name in ["delete", "remove_attribute"] {
        m.define_method(name, method!(mutate::delete, 1))
            .expect("#delete");
    }
    m.define_method("content=", method!(mutate::set_content, 1))
        .expect("#content=");
    m.define_method("name=", method!(mutate::set_name, 1))
        .expect("#name=");

    doc.define_method("create_element", method!(mutate::create_element, 1))
        .expect("#create_element");
    doc.define_method(
        "create_document_type",
        method!(mutate::create_document_type, -1),
    )
    .expect("#create_document_type");
    doc.define_method("create_text_node", method!(mutate::create_text_node, 1))
        .expect("#create_text_node");
    doc.define_method("create_comment", method!(mutate::create_comment, 1))
        .expect("#create_comment");
    doc.define_method(
        "create_processing_instruction",
        method!(mutate::create_pi, 2),
    )
    .expect("#create_processing_instruction");
    doc.define_method(
        "create_document_fragment",
        method!(mutate::create_document_fragment, 0),
    )
    .expect("#create_document_fragment");
}
