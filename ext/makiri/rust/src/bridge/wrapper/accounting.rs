//! What a Document reports to the GC of the memory it holds outside Ruby's
//! allocator.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;

use magnus::Value;

use super::*;
use crate::bridge::ruby::{value, VALUE};

/// Tell the GC how much memory `rb_doc` holds outside Ruby's allocator.
///
/// Neither the HTML arena nor the XML arena is an `xmalloc`, so the GC sees
/// a parsed document as a few dozen bytes: a loop that parses and drops never
/// triggers a collection from memory pressure, RSS climbs by a document per
/// parse, and every parse pays for freshly faulted pages (measured: 2× the
/// parse time, and gigabytes of RSS, on a 280 KB document). This reports the
/// difference since the last call, so it is safe to call again after the
/// document grows.
///
/// May run a collection right here, so `rb_doc` must be reachable from the
/// caller's frame (a local VALUE is), and nothing borrowed from a Ruby String
/// may be held across the call.
pub(super) fn account_document(rb_doc: VALUE) {
    // SAFETY: `rb_doc` is a Document (the base type matches either leaf),
    // live on the caller's frame.
    let rb_doc = unsafe { value(rb_doc) };
    let d = DOC_TYPE.get_known(&rb_doc);
    let (now, chunks) = (d.external_bytes(), d.arena_chunks());
    let mut report = d.report.get();
    let diff = report.update(now, chunks);
    d.report.set(report);
    /* After the cell is written: the report may run a collection, which
     * reads `d` shared - as it may, now that nothing holds it `&mut`. */
    if let Some(diff) = diff {
        crate::bridge::ruby::report_external_bytes(diff);
    }
}

/// Chunks an HTML document's pools must gain before [`account_growth`]
/// measures it again (the HTML arena's stores grow in chunks of tens of KiB).
pub(super) const GROWTH_MIN_CHUNKS: usize = 16;
/// Bytes an XML document must gain before [`account_growth`] reports again.
pub(super) const GROWTH_MIN_BYTES: usize = 512 * 1024;

/// Tell the GC about what `rb_doc` has grown by since its last report, once
/// that is enough to matter ([`DocData::grown_enough`]: an eighth, and a floor).
///
/// A parse reports its document's size once; mutation grows the arena after
/// that - nodes appended in a loop, `inner_html=` of a large string - and
/// without a new report the GC keeps judging a large document by its size at
/// parse time. Measuring an HTML document walks every chunk of its pools, too
/// costly for every edit, so an O(1) chunk count decides when; growth by a
/// fraction also keeps the measurements logarithmic in the final size.
///
/// Called where an edit begins (`bridge::html::edit`, `bridge::xml::
/// begin_edit`): the growth it sees is the previous edits', and a document's
/// last edit is reported at its next one or not at all - an under-report, the
/// safe direction, as it was before this existed. It may run a collection,
/// so, like [`account_document`], it must be called with `rb_doc` on the
/// caller's stack and no borrowed Ruby String held - which is so at the start
/// of an edit, before any argument is converted.
pub fn account_growth(rb_doc: Value) {
    if doc_data_known(&rb_doc).grown_enough() {
        account_document(rb_doc.as_raw());
    }
}

/// Whether `rb_doc` is an HTML Document whose kept document-order index is
/// built - read before an evaluate, for [`account_kept_order`] after it.
pub fn has_kept_order(rb_doc: Value) -> bool {
    with_html_parsed_if(rb_doc, |p| p.has_doc_order()).unwrap_or(false)
}

/// Tell the GC about the document-order index an evaluate on `rb_doc` built,
/// `had` being [`has_kept_order`] before it. The index is kept across
/// evaluates, as the arena is, so it is reported as the arena is; an edit
/// drops it, and the next report gives it back.
///
/// Called once the evaluate is over - its walk, its handler guard - since
/// the report may run a collection: like [`account_document`], with `rb_doc`
/// on the caller's stack and no borrowed Ruby String held.
pub fn account_kept_order(rb_doc: Value, had: bool) {
    if !had && has_kept_order(rb_doc) {
        account_document(rb_doc.as_raw());
    }
}
