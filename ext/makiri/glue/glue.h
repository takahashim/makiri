#ifndef MAKIRI_GLUE_H
#define MAKIRI_GLUE_H

#include "../makiri.h"
#include "../lexbor_compat/compat.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Wrapper for any DOM node except Document. The node memory is owned by the
 * document's Lexbor arena; we keep only the pointer plus a keepalive VALUE
 * reference to the Ruby Document so the arena outlives the wrapper. */
typedef struct {
    lxb_dom_node_t *node;
    VALUE           document;
} mkr_node_data_t;

/* Wrapper for a Document: owns the parsed result (and thus the arena). */
typedef struct {
    mkr_parsed_t *parsed;
    VALUE         errors;   /* Ruby Array; reserved for parse warnings */
} mkr_doc_data_t;

/* Hard cap on the number of nodes a single NodeSet may hold; mirrors the XPath
 * engine's max_nodeset_size so every node-collecting path (tree walks, XPath,
 * CSS) fails closed at the same bound. */
#define MKR_NODE_SET_MAX (10u * 1000u * 1000u)

extern const rb_data_type_t mkr_node_type;
extern const rb_data_type_t mkr_doc_type;
extern const rb_data_type_t mkr_node_set_type;

/* Node bridge (glue/ruby_node.c). mkr_wrap_node returns the Document VALUE
 * for the document node, Qnil for NULL, otherwise a freshly-wrapped Node. */
VALUE           mkr_wrap_node(lxb_dom_node_t *node, VALUE document);
lxb_dom_node_t *mkr_node_unwrap(VALUE rb_node);
VALUE           mkr_node_document(VALUE rb_node);

/* Document bridge (glue/ruby_doc.c). */
lxb_dom_document_t *mkr_doc_unwrap(VALUE rb_doc);
mkr_parsed_t       *mkr_doc_parsed(VALUE rb_doc);
VALUE               mkr_wrap_document(mkr_parsed_t *parsed); /* GC takes ownership */

/* Shared fragment-parse machinery (glue/ruby_doc.c), reused by ruby_mutate.c's
 * inner_html=/outer_html= so the UTF-8 sanitisation and import+template-fixup
 * are not duplicated.
 *
 * mkr_sanitize_html_input: decode rb_html for the fragment parser — *out/*out_len
 * are the bytes to parse, *owned a malloc'd buffer to free afterwards (NULL when
 * the input is used in place). Returns 0, or -1 on OOM (nothing allocated), so
 * the caller can release its parser before raising. See mkr_utf8_sanitize.
 *
 * mkr_import_fragment_children: deep-import each child of `root` into `doc`, hand
 * it to `emit`, and fix up any <template> contents (which import_node omits).
 *
 * mkr_emit_append / mkr_emit_before: emit callbacks — append as last child of
 * `u`, or insert before the reference node `u`. */
int  mkr_sanitize_html_input(VALUE html, const lxb_char_t **out, size_t *out_len,
                             lxb_char_t **owned);
void mkr_import_fragment_children(lxb_dom_document_t *doc, lxb_dom_node_t *root,
                                  void (*emit)(lxb_dom_node_t *, void *), void *u);
void mkr_emit_append(lxb_dom_node_t *imported, void *u);
void mkr_emit_before(lxb_dom_node_t *imported, void *u);

/* NodeSet bridge (glue/ruby_node_set.c). */
VALUE mkr_node_set_new(VALUE document);
void  mkr_node_set_push(VALUE rb_set, lxb_dom_node_t *node);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_GLUE_H */
