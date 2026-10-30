//! The Ruby <-> arena seam for HTML nodes: wrapping an arena node for Ruby
//! and back, the checked receiver (`HtmlSelf`), and the tree and attribute
//! edits the node methods make.
//!
//! A value read out of a Ruby wrapper becomes a live arena handle here,
//! through a checked lookup; the arena side (`html::dom`) stays Ruby-free.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, ReprValue, Value};
use crate::bridge::ruby::makiri_error;

use crate::html_dom::html::{
    ForeignNode, HtmlNode, HtmlNodeKey, HtmlTree, Insertion, NodeType, Place, RawNode,
};
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_DOCUMENT,
};

use crate::bridge::dom::{HtmlRead, HtmlWrite};
use crate::bridge::string::{utf8_str, RubyData, RubyText};
use crate::bridge::wrapper::*;
use crate::child_index::{ChildList, Children, TokenTree};
use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes};
use crate::html_dom::AdapterOom;

/// The HTML Document: parsing one (GVL-released), its readers, and
/// cross-document import/clone.
pub mod document;
/// The fragment pipeline: `Document#fragment`, `DocumentFragment.parse`,
/// `Node#parse`, `inner_html=` and `outer_html=`.
pub mod fragment;

mod attrs;
mod edit;
mod insert;
mod receiver;
mod wrap;

pub use attrs::*;
pub use edit::*;
pub use insert::*;
pub use receiver::*;
pub use wrap::*;
