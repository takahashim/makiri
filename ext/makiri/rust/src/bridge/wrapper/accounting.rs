//! What a Document reports to the GC of the memory it holds outside Ruby's
//! allocator.

#![forbid(unsafe_code)]

use crate::bridge::rb::Value;

use super::*;

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
/// May run a collection right here, with `rb_doc`'s data borrowed across it,
/// so `rb_doc` must be a Document Ruby keeps for the call - a method's
/// receiver, or a node's slot value - and not one only this frame refers to,
/// which a local `Value` does not keep (a new Document is reported through
/// its handle, [`account_data`]); nothing borrowed from a Ruby String may be
/// held across the call.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        unheld_payload_borrow,
        reason = "its contract: rb_doc is a receiver or a node's slot value, which Ruby keeps for the call (a new Document goes through account_data, over its handle)"
    )
)]
pub(super) fn account_document(ruby: &crate::bridge::rb::Ruby, rb_doc: Value) {
    account_data(ruby, DOC_TYPE.get_known(&rb_doc));
}

/// [`account_document`] over the wrapper's data, borrowed from a handle that
/// holds the object (`new_document`'s fill, for a Document nothing else
/// refers to yet).
pub(super) fn account_data(ruby: &crate::bridge::rb::Ruby, d: &DocData) {
    /* Content an edit holds is measured at the next report - which no
     * caller reaches with an edit open, since none runs Ruby. */
    let (Some(now), Some(chunks)) = (d.external_bytes(), d.arena_chunks()) else {
        return;
    };
    d.chunks.set(chunks);
    /* The report may run a collection, which reads `d` shared - as it may:
     * nothing holds it `&mut`. */
    d.external.report(ruby, now);
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
/// Called in two places. Where an edit begins (`bridge::html::edit`,
/// `bridge::xml::begin_edit`): the growth it sees is the previous edits', and
/// a document's last edit is reported at its next one or not at all - an
/// under-report, the safe direction. And AFTER a growth no edit follows - a
/// factory's node, an imported copy, a parsed fragment, an adoption's copy
/// (`bridge::html::lend_document` and `insert`, `bridge::xml::fragment_into`,
/// `import_copy` and `insert`) - once the document's loan is over. It may run
/// a collection, so, like [`account_document`], it must be called with
/// `rb_doc` on the caller's stack and no borrowed Ruby String held - which is
/// so at the start of an edit, before any argument is converted, and after a
/// loan, whose arguments were copies.
pub fn account_growth(ruby: &crate::bridge::rb::Ruby, rb_doc: Value) {
    if doc_data_known(&rb_doc).grown_enough() {
        account_document(ruby, rb_doc);
    }
}

/// Whether `rb_doc` is an HTML Document whose kept document-order index is
/// built - read before an evaluate, for [`account_kept_order`] after it. A
/// document that cannot be read now (an edit holds it) is the `Err` the
/// evaluate would meet too, passed on rather than read as "no index".
pub fn has_kept_order(rb_doc: Value) -> Result<bool, crate::bridge::rb::Error> {
    Ok(with_html_parsed_if(rb_doc, |p| p.has_doc_order())? == Some(true))
}

/// Tell the GC about the document-order index an evaluate on `rb_doc` built,
/// `had` being [`has_kept_order`] before it. The index is kept across
/// evaluates, as the arena is, so it is reported as the arena is; an edit
/// drops it, and the next report gives it back.
///
/// Called once the evaluate's walk is over, since
/// the report may run a collection: like [`account_document`], with `rb_doc`
/// on the caller's stack and no borrowed Ruby String held.
pub fn account_kept_order(
    ruby: &crate::bridge::rb::Ruby,
    rb_doc: Value,
    had: bool,
) -> Result<(), crate::bridge::rb::Error> {
    if !had && has_kept_order(rb_doc)? {
        account_document(ruby, rb_doc);
    }
    Ok(())
}
