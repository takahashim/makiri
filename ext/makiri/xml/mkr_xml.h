/* mkr_xml.h — public boundary of the native XML reader (Phase 1, read-only).
 *
 * Ruby-free. The Ruby surface (Makiri::XML(...) / Makiri.parse_xml) lives in
 * glue/ruby_xml.c, which decodes input (bridge/mkr_xml_decode_input), releases
 * the GVL, and calls mkr_xml_parse here. See docs/xml_parser_plan.ja.md.
 */
#ifndef MKR_XML_H
#define MKR_XML_H

#include <stddef.h>
#include "mkr_xml_node.h"

/* Fail-closed status codes (mapped to Ruby exceptions at the glue boundary). */
typedef enum {
    MKR_XML_OK = 0,
    MKR_XML_ERR_SYNTAX,   /* well-formedness violation -> Makiri::XML::SyntaxError */
    MKR_XML_ERR_LIMIT,    /* budget exceeded            -> Makiri::XML::LimitExceeded */
    MKR_XML_ERR_OOM       /* allocation failure         -> Makiri::Error */
} mkr_xml_status_t;

/* Per-document budgets (§4). Enforced in the arena / tree builder; exceeding any
 * of them is fail-closed (no partial/truncated document — raise instead). */
#define MKR_XML_MAX_DEPTH   1024u                 /* element nesting (no stack DoS) */
#define MKR_XML_MAX_NODES   (10u * 1000u * 1000u) /* total DOM nodes */
#define MKR_XML_MAX_ATTRS   4096u                 /* attributes per element */
#define MKR_XML_MAX_NS      4096u                 /* distinct namespaces per doc */
#ifndef MKR_XML_MAX_BYTES
#define MKR_XML_MAX_BYTES   ((size_t)-1)          /* arena payload cap (effectively off) */
#endif

/* Parse +len+ bytes of already-decoded, validated UTF-8 (§2.1 guarantees the
 * input is valid UTF-8, XML-Char-only, NUL-free) into a document. Ruby-free, so
 * the caller runs it with the GVL released. Returns NULL and sets *status on
 * failure; otherwise an owned mkr_xml_doc_t the caller must hand to GC
 * (mkr_xml_doc_destroy on free).
 *
 * SKELETON (this commit): builds an empty document — the tokenizer and tree
 * builder land in the next steps. The decode -> GVL -> arena -> doc -> wrap
 * pipeline is exercised end to end. */
mkr_xml_doc_t *mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status);

#endif /* MKR_XML_H */
