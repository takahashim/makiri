/* ruby_xml.c — Ruby boundary for the native XML reader (Phase 1).
 *
 * Makiri::XML(source) / Makiri.parse_xml(source): strict-decode the input
 * (§2.1), then run the Ruby-free parser with the GVL released, and return a
 * Makiri::XML::Document. The document is held in a kind=MKR_DOC_XML mkr_parsed_t
 * (the common document handle, §2.3) and wrapped by mkr_wrap_document, which GC
 * frees via mkr_parsed_destroy (the XML branch whole-arena-frees).
 */
#include "../makiri.h"
#include "../core/mkr_core.h"
#include "../xml/mkr_xml.h"
#include "glue.h"   /* mkr_wrap_document, mkr_parsed_* (via compat.h) */

#include <ruby/thread.h>

/* ---- GVL-released parse ---- */
typedef struct {
    const char      *src;
    size_t           len;
    mkr_xml_doc_t   *result;
    mkr_xml_status_t status;
} mkr_xml_parse_nogvl_t;

static void *
mkr_xml_parse_nogvl(void *p)
{
    mkr_xml_parse_nogvl_t *a = (mkr_xml_parse_nogvl_t *)p;
    a->result = mkr_xml_parse(a->src, a->len, &a->status);
    return NULL;
}

/* call-seq: Makiri.parse_xml(source) -> Makiri::XML::Document
 *           Makiri::XML(source)      -> Makiri::XML::Document */
static VALUE
mkr_xml_s_parse(VALUE self, VALUE rb_source)
{
    (void)self;
    /* Strict decode under the GVL: invalid UTF-8 / undecodable byte / NUL all
     * raise Makiri::XML::SyntaxError here (no U+FFFD repair). */
    VALUE decoded = mkr_xml_decode_input(rb_String(rb_source));

    /* Build an empty XML handle and wrap it first (doc == NULL) so a failure
     * mid-parse frees cleanly via GC (mkr_parsed_destroy -> the XML branch ->
     * mkr_xml_doc_destroy(NULL), a no-op). */
    mkr_parsed_t *parsed = mkr_parsed_new_xml(NULL);
    if (parsed == NULL) {
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    VALUE obj = mkr_wrap_document(parsed); /* GC owns +parsed+ from here */

    /* Copy the decoded bytes so the parse can run with the GVL released without
     * racing GC/compaction on the String's backing store. */
    mkr_owned_bytes_t source = {0};
    if (mkr_ruby_copy_bytes(decoded, &source) != 0) {
        rb_raise(mkr_eError, "out of memory copying XML source");
    }
    RB_GC_GUARD(decoded);

    mkr_xml_parse_nogvl_t args = { source.ptr, source.len, NULL, MKR_XML_OK };
    rb_thread_call_without_gvl(mkr_xml_parse_nogvl, &args, NULL, NULL);
    mkr_owned_bytes_clear(&source);

    if (args.result == NULL) {
        switch (args.status) {
        case MKR_XML_ERR_SYNTAX: rb_raise(mkr_eXmlSyntaxError,   "malformed XML"); break;
        case MKR_XML_ERR_LIMIT:  rb_raise(mkr_eXmlLimitExceeded, "XML document budget exceeded"); break;
        default:                 rb_raise(mkr_eError,            "failed to parse XML document"); break;
        }
    }
    mkr_parsed_set_xml_doc(parsed, args.result);
    RB_GC_GUARD(obj);
    return obj;
}

void
mkr_init_xml(void)
{
    /* Standalone class for now; the per-kind hierarchy (Makiri::Document abstract
     * base + HTML/XML leaves, §12) lands in step 1b. */
    mkr_cXmlDocument = rb_define_class_under(mkr_mXML, "Document", rb_cObject);
    rb_undef_alloc_func(mkr_cXmlDocument); /* created only from C, never .new */

    rb_define_module_function(mkr_mMakiri, "parse_xml", mkr_xml_s_parse, 1);
    /* Makiri::XML(source) — a method named XML on the Makiri module, coexisting
     * with the Makiri::XML constant (the module), as Nokogiri::XML does. */
    rb_define_module_function(mkr_mMakiri, "XML", mkr_xml_s_parse, 1);
}
