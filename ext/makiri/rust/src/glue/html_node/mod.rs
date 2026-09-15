//! The HTML (Lexbor) node representation (glue/ruby_html_node.c).
//!
//! Wrapping an `lxb_dom_node_t` into a `Makiri::HTML::*` leaf, the HTML
//! node-pointer accessor, and the reader methods that hang off
//! `Makiri::HTML::NodeMethods`. The XML counterpart is `glue::xml_node`; the
//! representation-neutral node core - the `rb_data_type_t` chain and the
//! kind-agnostic accessors - is `glue::node`.
//!
//! # Two functions here are the HTML node's front door
//!
//! [`wrap_html_node`] and [`html_node_unwrap`] are how every other glue
//! module wraps and unwraps an HTML node. `glue::abi`'s `agree` module pins their
//! signatures, so a change to either is a visible one.
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
use core::ptr::NonNull;

use magnus::rb_sys::FromRawValue;
use magnus::{method, prelude::*, RClass, Ruby, Value};
use rb_sys::VALUE;

use super::abi::{
    html_doc_unwrap, html_node_methods, is_kind_of, LxbNode, NodeData, CLASS_DOCUMENT,
    CLASS_XML_DOCUMENT,
};
/* Only the mutation half registers on the Document class. */
use super::abi::CLASS_HTML_DOCUMENT;
use crate::dom_adapter::html::HtmlNode;

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

pub use crate::glue::doc::node_clone_node;
pub use crate::glue::node::node_equals;
pub use crate::glue::node::node_hash;
pub use crate::glue::node::node_pointer_id;
pub use crate::glue::node::HTML_NODE_TYPE;
pub use crate::init::CLASS_HTML_ATTR;
pub use crate::init::CLASS_HTML_CDATA_SECTION;
pub use crate::init::CLASS_HTML_COMMENT;
pub use crate::init::CLASS_HTML_DOCUMENT_FRAGMENT;
pub use crate::init::CLASS_HTML_DOCUMENT_TYPE;
pub use crate::init::CLASS_HTML_ELEMENT;
pub use crate::init::CLASS_HTML_NODE;
pub use crate::init::CLASS_HTML_PROCESSING_INSTRUCTION;
pub use crate::init::CLASS_HTML_TEXT;

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
pub unsafe extern "C" fn wrap_html_node(node: *mut LxbNode, document: VALUE) -> VALUE {
    let Some(handle) = HtmlNode::from_raw(node) else {
        return rb_sys::Qnil as VALUE;
    };
    let node_type = handle.node_type();
    if node_type == ty::DOCUMENT {
        return document;
    }

    let klass = match node_type {
        ty::ELEMENT => CLASS_HTML_ELEMENT.raw(),
        ty::ATTRIBUTE => CLASS_HTML_ATTR.raw(),
        ty::TEXT => CLASS_HTML_TEXT.raw(),
        ty::COMMENT => CLASS_HTML_COMMENT.raw(),
        ty::CDATA => CLASS_HTML_CDATA_SECTION.raw(),
        ty::PI => CLASS_HTML_PROCESSING_INSTRUCTION.raw(),
        ty::DOCTYPE => CLASS_HTML_DOCUMENT_TYPE.raw(),
        ty::FRAGMENT => CLASS_HTML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_HTML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `wrap_zeroed`. */
    crate::bridge::ruby::wrap_zeroed::<NodeData>(
        klass,
        HTML_NODE_TYPE.as_ptr(),
        |nd| nd.node = node as *mut c_void,
        |nd| nd.document = document,
    )
}

/// The `lxb_dom_node_t` behind an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is
/// against `HTML_NODE_TYPE`, which an XML node - wrapped under
/// `XML_NODE_TYPE` - does not satisfy. Every HTML-glue site that
/// dereferences a node or hands its pointer to Lexbor goes through here, for
/// `self` and arguments alike.
pub unsafe fn html_node_unwrap(rb_node: VALUE) -> Result<*mut LxbNode, magnus::Error> {
    if is_kind_of(Value::from_raw(rb_node), &CLASS_DOCUMENT) {
        if is_kind_of(Value::from_raw(rb_node), &CLASS_XML_DOCUMENT) {
            return Err(magnus::Error::new(
                magnus::Ruby::get_unchecked().exception_type_error(),
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        return Ok(html_doc_unwrap(rb_node)? as *mut LxbNode);
    }
    let nd = crate::bridge::ruby::typed_data(Value::from_raw(rb_node), &HTML_NODE_TYPE)?
        as *mut NodeData;
    Ok((*nd).node as *mut LxbNode)
}

/* ---- the Rust-side conveniences the reader module uses ---- */

/// [`html_node_unwrap`] in Rust terms.
pub unsafe fn unwrap(v: Value) -> Result<*mut LxbNode, magnus::Error> {
    use magnus::rb_sys::AsRawValue;
    html_node_unwrap(v.as_raw())
}

/// A method receiver already checked to be an HTML node or HTML Document.
///
/// The check runs as magnus converts the receiver, so a reader bound onto the
/// wrong kind of node (the differential's `html_reader_on_xml`) fails with the
/// same TypeError before the method body starts.
#[derive(Clone, Copy)]
pub struct HtmlSelf {
    pub value: Value,
    raw: NonNull<LxbNode>,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl magnus::TryConvert for HtmlSelf {
    fn try_convert(value: Value) -> Result<Self, magnus::Error> {
        // SAFETY: magnus converts the receiver under the GVL.
        unsafe {
            use magnus::rb_sys::AsRawValue;
            let raw = NonNull::new(unwrap(value)?).ok_or_else(uninitialized)?;
            let document = Value::from_raw(super::abi::keepalive_document(value.as_raw())?);
            Ok(HtmlSelf {
                value,
                raw,
                document,
            })
        }
    }
}

fn uninitialized() -> magnus::Error {
    magnus::Error::new(
        magnus::Ruby::get()
            .expect("under the GVL")
            .exception_type_error(),
        "uninitialized HTML node",
    )
}

impl HtmlSelf {
    /// The receiver's node, for the length of this method call.
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call, and it keeps its document alive; the borrow of `self` ends the
    /// handle with the call. Like a magnus `Value`, an `HtmlSelf` is only ever
    /// held on the stack of the method it was converted for.
    #[inline]
    pub fn node(&self) -> HtmlNode<'_> {
        // SAFETY: as above; the document is not restructured by a reader, and
        // a mutator refuses while an XPath handler could be reading it.
        unsafe { HtmlNode::from_raw(self.raw.as_ptr()) }.expect("non-null by construction")
    }

    /// The receiver's node as Lexbor's handle, for the mutators.
    #[inline]
    pub fn raw(&self) -> *mut LxbNode {
        self.raw.as_ptr()
    }
}

/// An HTML node argument, for the length of the borrow of `v`.
///
/// `Err(TypeError)` for anything that is not an HTML node or HTML Document.
/// The same reasoning as [`HtmlSelf::node`]: `v` is a method argument on the
/// stack, which keeps its node's document alive.
pub fn arg_node(v: &Value) -> Result<HtmlNode<'_>, magnus::Error> {
    // SAFETY: as above.
    unsafe { HtmlNode::from_raw(unwrap(*v)?) }.ok_or_else(uninitialized)
}

/// [`wrap`] for an optional handle: nil for None.
///
/// # Safety
/// `document` must be the keepalive Document of `node`'s tree.
#[inline]
pub unsafe fn wrap_node(node: Option<HtmlNode<'_>>, document: Value) -> Value {
    wrap(
        node.map_or(core::ptr::null_mut(), HtmlNode::as_raw),
        document,
    )
}

pub unsafe fn wrap(node: *mut LxbNode, document: Value) -> Value {
    use magnus::rb_sys::AsRawValue;
    Value::from_raw(wrap_html_node(node, document.as_raw()))
}

/// The keepalive Document of a node, from the kind-agnostic accessor.
pub unsafe fn node_document(v: Value) -> Result<Value, magnus::Error> {
    use magnus::rb_sys::AsRawValue;
    Ok(Value::from_raw(super::abi::keepalive_document(v.as_raw())?))
}

/* ------------------------------------------------------------------ *
 * registration                                                       *
 * ------------------------------------------------------------------ */

/// The shape `rb_define_method` wants. Ruby dispatches on the declared arity,
/// so every arity is reached through this one type.
type RbMethod = unsafe extern "C" fn() -> VALUE;

/// Bind a method implemented with the C calling convention: the identity
/// methods shared with the XML side, and `clone_node`.
unsafe fn define_c_method(module: VALUE, name: &core::ffi::CStr, f: RbMethod, arity: i32) {
    rb_sys::rb_define_method(module, name.as_ptr(), Some(f), arity);
}

/// `init_node` - the HTML node surface.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn init_node() {
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
        core::mem::transmute(node_equals as unsafe extern "C" fn(VALUE, VALUE) -> VALUE);
    let hash: RbMethod = core::mem::transmute(node_hash as unsafe extern "C" fn(VALUE) -> VALUE);
    let ptr_id: RbMethod =
        core::mem::transmute(node_pointer_id as unsafe extern "C" fn(VALUE) -> VALUE);
    let clone: RbMethod = core::mem::transmute(
        node_clone_node as unsafe extern "C" fn(core::ffi::c_int, *const VALUE, VALUE) -> VALUE,
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
    let dt =
        RClass::from_value(CLASS_HTML_DOCUMENT_TYPE.value()).expect("Makiri::HTML::DocumentType");
    for name in ["public_id", "external_id"] {
        dt.define_method(name, method!(read::doctype_public_id, 0))
            .expect("#public_id");
    }
    dt.define_method("system_id", method!(read::doctype_system_id, 0))
        .expect("#system_id");

    /* <template> contents (WHATWG DOM HTMLTemplateElement.content). */
    let el = RClass::from_value(CLASS_HTML_ELEMENT.value()).expect("Makiri::HTML::Element");
    el.define_method("content_fragment", method!(read::content_fragment, 0))
        .expect("#content_fragment");
}

use magnus::rb_sys::AsRawValue;

/// `init_mutate` - the HTML node's mutators and the Document factories.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub unsafe extern "C" fn init_mutate() {
    let m = html_node_methods();
    let doc = RClass::from_value(CLASS_HTML_DOCUMENT.value()).expect("HTML::Document");

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
