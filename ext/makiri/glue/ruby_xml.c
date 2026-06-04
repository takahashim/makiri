/* ruby_xml.c — Ruby boundary for the native XML reader (Phase 1).
 *
 * Makiri::XML(source) / Makiri.parse_xml(source): strict-decode the input
 * (§2.1), then run the Ruby-free parser with the GVL released, and return a
 * Makiri::XML::Document wrapping the owned document arena (GC frees it).
 *
 * SKELETON: mkr_xml_parse currently builds an empty document; the tokenizer and
 * tree builder land next. This validates the decode -> GVL -> arena -> doc ->
 * wrap pipeline end to end.
 */
#include "../makiri.h"
#include "../core/mkr_core.h"
#include "../xml/mkr_xml.h"

#include <ruby/thread.h>

/* Class ref kept alive by the Makiri::XML::Document constant. */
static VALUE mkr_cXmlDocument;

/* ---- Makiri::XML::Document wrapper (owns the arena) ---- */
typedef struct {
    mkr_xml_doc_t *doc;
} mkr_xml_doc_data_t;

static void
mkr_xml_doc_dfree(void *ptr)
{
    mkr_xml_doc_data_t *d = (mkr_xml_doc_data_t *)ptr;
    mkr_xml_doc_destroy(d->doc); /* whole-arena free; safe on NULL */
    xfree(d);
}

static size_t
mkr_xml_doc_dsize(const void *ptr)
{
    const mkr_xml_doc_data_t *d = (const mkr_xml_doc_data_t *)ptr;
    return sizeof(*d) + mkr_xml_doc_memsize(d->doc);
}

static const rb_data_type_t mkr_xml_doc_type = {
    "Makiri::XML::Document",
    { NULL, mkr_xml_doc_dfree, mkr_xml_doc_dsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

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

    /* Allocate the wrapper first (doc == NULL) so a failure mid-parse frees
     * cleanly via GC. */
    mkr_xml_doc_data_t *d;
    VALUE obj = TypedData_Make_Struct(mkr_cXmlDocument, mkr_xml_doc_data_t,
                                      &mkr_xml_doc_type, d);
    d->doc = NULL;

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

    d->doc = args.result;
    if (d->doc == NULL) {
        switch (args.status) {
        case MKR_XML_ERR_SYNTAX: rb_raise(mkr_eXmlSyntaxError,   "malformed XML"); break;
        case MKR_XML_ERR_LIMIT:  rb_raise(mkr_eXmlLimitExceeded, "XML document budget exceeded"); break;
        default:                 rb_raise(mkr_eError,            "failed to parse XML document"); break;
        }
    }
    return obj;
}

void
mkr_init_xml(void)
{
    mkr_cXmlDocument = rb_define_class_under(mkr_mXML, "Document", rb_cObject);
    rb_undef_alloc_func(mkr_cXmlDocument); /* created only from C, never .new */

    rb_define_module_function(mkr_mMakiri, "parse_xml", mkr_xml_s_parse, 1);
    /* Makiri::XML(source) — a method named XML on the Makiri module, coexisting
     * with the Makiri::XML constant (the module), as Nokogiri::XML does. */
    rb_define_module_function(mkr_mMakiri, "XML", mkr_xml_s_parse, 1);
}
