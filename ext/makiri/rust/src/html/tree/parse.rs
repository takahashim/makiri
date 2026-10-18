//! Parsing a whole document into a new arena document.

use super::{TreeBuilder, TreeError, TreeStatus};
use crate::html::dom::{Document, DomOom, NodeId};
use crate::html::res::names as res;
use crate::html::tokenizer::{Status, Tokenizer};

/// Why a parse produced no document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseFailure {
    /// An allocation failed.
    Oom,
    /// The tree builder stopped the parse.
    Stopped(TreeStatus),
}

/// A parsed document and the tree builder's parse errors.
pub struct Parsed {
    pub doc: Document,
    pub errors: Vec<TreeError>,
}

/// `lxb_html_document_parse`: tokenize `input` - valid UTF-8, the whole
/// document - and build its tree.
pub fn parse_document(input: &[u8]) -> Result<Parsed, ParseFailure> {
    let mut doc = Document::new(0).map_err(|DomOom| ParseFailure::Oom)?;
    /* An HTML document node is in the HTML namespace
     * (`lxb_dom_document_init`). */
    doc.set_names(NodeId::DOCUMENT, res::TAG__DOCUMENT, res::NS_HTML as u32, 0);
    let mut tree = TreeBuilder::new(&mut doc);
    let status = {
        let mut tkz = Tokenizer::new(input, &mut tree);
        /* `lxb_html_parser_init` */
        tkz.keep_duplicate = true;
        match tkz.chunk() {
            Status::Ok => tkz.end(),
            st => st,
        }
    };
    /* `lxb_html_parse_chunk_end`: whatever is still open is popped, with
     * its pop steps. */
    tree.open_pop_all();
    let errors = core::mem::take(&mut tree.errors);
    let tree_status = tree.status;
    drop(tree);
    match status {
        Status::Ok => Ok(Parsed { doc, errors }),
        Status::Oom => Err(ParseFailure::Oom),
        _ => Err(ParseFailure::Stopped(tree_status)),
    }
}
