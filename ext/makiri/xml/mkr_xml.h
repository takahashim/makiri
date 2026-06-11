/* mkr_xml.h - public boundary of the native XML reader (Phase 1, read-only).
 *
 * Ruby-free. The Ruby surface (Makiri::XML::Document.parse / Makiri::XML(...)) lives in
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
    MKR_XML_ERR_OOM,      /* allocation failure         -> Makiri::Error */
    MKR_XML_ERR_INTERNAL, /* impossible-state guard tripped (programming error) -> Makiri::Error */
    MKR_XML_ERR_VERSION   /* well-formed but a version != 1.0 (XML 1.1) -> Makiri::XML::SyntaxError */
} mkr_xml_status_t;

/* Per-document budgets (§4). Enforced in the arena / tree builder; exceeding any
 * of them is fail-closed (no partial/truncated document - raise instead). */
#define MKR_XML_MAX_DEPTH   1024u                 /* element nesting (no stack DoS) */
#define MKR_XML_MAX_NODES   (10u * 1000u * 1000u) /* total DOM nodes - a SECONDARY guard; the
                                                   * byte budget below is the primary memory
                                                   * ceiling (at ~128 B/node it caps node count
                                                   * well under 10M, so it trips first). */
#define MKR_XML_MAX_ATTRS   4096u                 /* attributes per element */
#define MKR_XML_MAX_NS      4096u                 /* namespace bindings IN SCOPE at once - the
                                                   * scope-stack depth, popped on element close.
                                                   * Bounds concurrent bindings + the binds array,
                                                   * NOT document-wide distinct declarations (those
                                                   * are each an attribute, bounded by the byte /
                                                   * attr budgets). */
#ifndef MKR_XML_MAX_BYTES
/* Arena payload cap: the single arena choke (arena_alloc) counts BOTH node
 * structs and copied name/value bytes against this, so it is the master memory
 * ceiling - it subsumes the node-count limit and bounds tiny-element
 * amplification (a `<a/>` flood is ~23x input→arena, measured) to ~11 MB of
 * hostile input. Enforced before each allocation, and the parse entry rejects
 * any input longer than this up front (so the line-ending scratch is bounded
 * too). 256 MiB comfortably fits every standard document (a 50 MB sitemap is
 * ~82 MB of arena; RSS/Atom/SOAP far less) and only rejects documents past
 * ~2M elements. A caller that needs more passes max_bytes to mkr_xml_parse_ex
 * (per-document override); -DMKR_XML_MAX_BYTES changes the default. */
#define MKR_XML_MAX_BYTES   ((size_t)256u * 1024u * 1024u)  /* 256 MiB */
#endif

/* Per-parse budget overrides (the runtime counterpart of the MKR_XML_MAX_*
 * compile-time defaults). A 0 field means "use the compile-time default", so a
 * zeroed struct (or a NULL pointer to mkr_xml_parse_ex) reproduces mkr_xml_parse
 * exactly. Only max_bytes is honoured today; the other budgets (max_nodes,
 * max_depth, max_attrs, max_ns) are intended to join this struct as they become
 * runtime-configurable, without changing the entry-point signature. */
typedef struct {
    size_t max_bytes;   /* arena payload cap; 0 = MKR_XML_MAX_BYTES */
} mkr_xml_limits_t;

/* Parse +len+ bytes of already-decoded, validated UTF-8 (§2.1 guarantees the
 * input is valid UTF-8, XML-Char-only, NUL-free) into a document. Ruby-free, so
 * the caller runs it with the GVL released. Returns NULL and sets *status on
 * failure; otherwise an owned mkr_xml_doc_t the caller must hand to GC
 * (mkr_xml_doc_destroy on free).
 *
 * mkr_xml_parse uses the compile-time default budgets; mkr_xml_parse_ex applies
 * the non-zero fields of +limits+ over them first (a NULL +limits+ is identical
 * to mkr_xml_parse). */
mkr_xml_doc_t *mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status);
mkr_xml_doc_t *mkr_xml_parse_ex(const char *src, size_t len,
                                const mkr_xml_limits_t *limits, mkr_xml_status_t *status);

/* Parse +src+ as a FRAGMENT (0+ top-level nodes: char data, multiple elements,
 * comments, PIs, CDATA - no XML declaration, DOCTYPE, or single-root rule) into a
 * fresh DOCUMENT_FRAGMENT node in +doc+'s arena, returned on success. When
 * +inherit_doc_ns+ is set, +doc+'s root in-scope namespace declarations seed the
 * scope so a prefixed/default-namespaced name resolves against the document
 * (Document#fragment); otherwise the fragment is self-contained
 * (DocumentFragment.parse, typically into a fresh doc). Bytes count against
 * +doc+'s budget. NULL + *status on error; the partial fragment is left detached
 * in the arena (never freed). The input must already be validated UTF-8. */
mkr_xml_node_t *mkr_xml_parse_fragment(mkr_xml_doc_t *doc, const char *src, size_t len,
                                       int inherit_doc_ns, mkr_xml_status_t *status);

/* Structural self-test of the tokenizer + tree builder (run from
 * Makiri.__c_selftest). Returns 0 on success or the 1-based failing check. */
int mkr_xml_parse_selftest(void);

/* Self-test of the monomorphized XML XPath engine instance (xpath/
 * mkr_xpath_xml_selftest.c): runs queries through the _xml engine over a small
 * document. Returns 0 or the 1-based failing check. */
int mkr_xml_xpath_selftest(void);

/* --- character data: XML Char + entity/char-reference expansion (§9.1/§9.2) --- */
int mkr_xml_is_char(uint32_t c);

/* Validate that [src, src+len) is entirely XML 1.0 Char with NO reference
 * recognition (comment/CDATA/PI content, where '&'/'<' are literal). 0 / -1. */
int mkr_xml_validate_chars(const char *src, uint32_t len);

/* XML 1.0 §2.3 NameStartChar / NameChar (§9.2b). One-codepoint decoding is the
 * core mkr_utf8_decode1 / mkr_utf8_decode1_span (strict, bounds-checked). */
int mkr_xml_is_name_start(uint32_t c);
int mkr_xml_is_name_char(uint32_t c);

/* Expansion context. ATTR adds XML 1.0 §3.3.3 attribute-value normalization:
 * a *literal* white-space byte (TAB/LF/CR) becomes a space, while a white-space
 * character produced by a numeric reference (&#9;/&#10;/&#13;) is preserved. TEXT
 * copies content verbatim (no whitespace folding). */
typedef enum {
    MKR_XML_EXPAND_TEXT = 0,
    MKR_XML_EXPAND_ATTR = 1
} mkr_xml_expand_mode_t;

/* Expand the 5 predefined entities + numeric char refs in [src, src+len),
 * validating XML Char (and, in ATTR mode, normalizing literal whitespace); writes
 * the (never-longer) result into the arena. Returns the arena slice and sets
 * *out_len, or NULL on undefined entity / bad reference / non-XML-Char (sets
 * *status) or arena OOM/LIMIT. */
const char *mkr_xml_expand(mkr_xml_doc_t *doc, const char *src, uint32_t len,
                           mkr_xml_expand_mode_t mode,
                           uint32_t *out_len, mkr_xml_status_t *status);

#endif /* MKR_XML_H */
