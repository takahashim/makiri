//! The XML Document: parsing one (GVL-released), its readers, a whole-document
//! copy, and a fragment parsed into one.

#![forbid(unsafe_code)]

use super::*;

/// Parse XML source, releasing the GVL, and wrap the result as a Document.
///
/// Runs the strict decode under the GVL first (invalid UTF-8, an undecodable
/// byte or a NUL all raise), which leaves the document in a buffer of its
/// own: the parse reads that, and no Ruby String, with the GVL released.
pub fn parse_xml_document(
    ruby: &Ruby,
    source: RString,
    limits: ParseLimits,
) -> Result<Value, Error> {
    let src = xml_decode_input(ruby, source, Some(limits.budget()))?;
    /* The wrapper before the parse: its allocation can fail, and `src`, an
     * owned copy, is dropped on the way out if it does. */
    new_xml_document(ruby, || {
        /* Ruby-free from here: only the copied bytes and the limits cross. */
        let bytes = src.as_slice();
        let result = crate::bridge::gvl::without_gvl(ruby, || {
            crate::xml::tree::parse_ex(bytes, Some(&limits))
        })?;
        /* Gone before the Document's report to the GC, which may collect. */
        drop(src);
        result.map_err(|status| parse_status_error(status, Unit::Document))
    })
    .map(|doc| doc.value())
}

/// `Document#root` for an XML document: the root element, or nil.
pub fn document_root(ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    let root = xml_read(&doc_known(rb_self))?.root();
    root.map(|n| wrap_xml_node(ruby, n, rb_self)).transpose()
}

/// `Document#internal_subset` for an XML document: the DOCTYPE node, or nil.
pub fn document_internal_subset(ruby: &Ruby, rb_self: Value) -> Result<Option<Value>, Error> {
    let doctype = xml_read(&doc_known(rb_self))?.doctype();
    doctype.map(|n| wrap_xml_node(ruby, n, rb_self)).transpose()
}

/// `Document#_copy`: a whole-document copy of `document`, node for node
/// (`mutate::copy_document`, where what it keeps is stated).
pub fn copy_xml_document(ruby: &Ruby, document: Value) -> Result<Value, Error> {
    /* The wrapper first, while nothing needs freeing (`new_document`). The
     * copy then reads `document` and runs no Ruby. */
    new_xml_document(ruby, || {
        crate::xml::mutate::copy_document(&*xml_read(&doc_known(document))?).map_err(xml_mut_error)
    })
    .map(|doc| doc.value())
}

/// A fresh, empty XML Document: an arena holding a DOCUMENT node and no root.
/// The handle holds it for a caller that fills it before handing it out
/// (`DocumentFragment.parse`).
pub fn new_empty_xml_document(ruby: &Ruby) -> Result<DocHandle, Error> {
    new_xml_document(ruby, || {
        XmlDoc::create(None).map_err(|_| makiri_error("out of memory allocating XML document"))
    })
}

/// Strict-decode `source` and parse it as a fragment into `document`'s arena,
/// returning the fragment node.
///
/// This runs UNDER the GVL on purpose: a fragment is small, and an existing
/// document's arena must never be mutated with the GVL released.
pub fn fragment_into(
    ruby: &Ruby,
    document: Value,
    source: RString,
    inherit_doc_ns: bool,
) -> Result<NodeId, Error> {
    let handle = doc_known(document);
    let max_bytes = xml_read(&handle)?.max_bytes();
    let src = xml_decode_input(ruby, source, Some(max_bytes))?;
    let parsed = {
        let mut xdoc = xml_write(&handle)?;
        tree::parse_fragment(&mut xdoc, src.as_slice(), inherit_doc_ns)
    };
    let frag = parsed.map_err(|status| parse_status_error(status, Unit::Fragment))?;
    /* The fragment grew the arena with no edit after it (a failed parse
     * rewound what it took), so it reports here, after the loan, as
     * `bridge::html`'s factories do (it may run a collection; the fragment
     * is a bare node of the arena, which `document` keeps). */
    crate::bridge::wrapper::account_growth(ruby, document);
    Ok(frag)
}
