//! The Ruby <-> XML-arena DOM seam (the XML counterpart of [`crate::bridge::html`]).
//!
//! A Ruby XML node is an arena [`NodeId`] behind a TypedData wrapper; turning a
//! `Value` into that id, and running the Ruby-free mutation primitives over the
//! arena, is the same boundary the HTML side has - so it lives here, under
//! `bridge`, and the glue layer stays free of it.
//!
//! The rules themselves (name well-formedness, the XML character class,
//! namespace resolution, what may be a child of what) live in the Ruby-free
//! `crate::xml`; this layer coerces and verifies arguments, and maps the
//! resulting status to a Ruby exception.
//!
//! **Detach, never destroy.** A removed node is unlinked, not freed, so a live
//! Ruby wrapper for it stays valid; the arena owns the memory and outlives every
//! wrapper through the Document.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::{prelude::*, Error, RString, Value};
use crate::bridge::ruby::makiri_error;

use crate::bridge::dom::{XmlRead, XmlWrite};
use crate::bridge::html::with_arg_node;
use crate::bridge::ruby::is_kind_of;
use crate::bridge::string::{ruby_verified_text, RubyText};
use crate::bridge::wrapper::*;
use crate::bridge::wrapper::{new_xml_document, node_repr, DocHandle, NodeRepr};
use crate::child_index::{ChildList, Children, TokenTree};
use crate::html_dom::cross_import::cross_html_to_xml;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT, EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::init::{
    CLASS_XML_ATTR, CLASS_XML_CDATA_SECTION, CLASS_XML_COMMENT, CLASS_XML_DOCUMENT_FRAGMENT,
    CLASS_XML_DOCUMENT_TYPE, CLASS_XML_ELEMENT, CLASS_XML_NODE, CLASS_XML_PROCESSING_INSTRUCTION,
    CLASS_XML_TEXT,
};
use crate::node_type::NodeType as CrateKind;
use crate::xml::attr_key::AttrKey;
use crate::xml::model::{ArenaKind, Document as XmlDoc, MutError, NodeId, ParseError, ParseLimits};
use crate::xml::mutate::{clone_node, copy_node_from, import_subtree, remove as remove_node};
use crate::xml::qname::NsDeclError;
use crate::xml::tree;
use decode::xml_decode_input;

/// The XML input's encoding chosen (XML 1.0 Appendix F) and the strict decode
/// to UTF-8 the reader runs before it sees a byte.
pub mod decode;

mod attrs;
mod document;
mod edit;
mod error;
mod insert;
mod receiver;
mod strings;
mod wrap;

pub use attrs::*;
pub use document::*;
pub use edit::*;
pub use error::*;
pub use insert::*;
pub use receiver::*;
pub use strings::*;
pub use wrap::*;
