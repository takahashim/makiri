#ifndef MAKIRI_COMPAT_H
#define MAKIRI_COMPAT_H

#include <stdbool.h>
#include <stddef.h>

#include <lexbor/html/html.h>
#include <lexbor/dom/dom.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Result of a parse. Owns the Lexbor document arena. The compat indices
 * (attr_owner, newline_idx) are created lazily and are NULL until then;
 * mkr_parsed_destroy frees whatever is set. */
typedef struct mkr_parsed_s {
    lxb_html_document_t *doc;
    void                *attr_owner;   /* M4: lxb_dom_attr_t -> owner element */
    void                *newline_idx;  /* M6: byte offset -> source line */
} mkr_parsed_t;

/* Parse HTML5 from src[0..len). Returns NULL on allocation or parse failure
 * (fail-closed). Source locations for Node#line are always tracked via the
 * tokenizer (cheap — it rides the parse). The src buffer is not retained:
 * Lexbor copies what it needs into the arena, and we build the line table up
 * front. */
mkr_parsed_t *mkr_parse_html(const lxb_char_t *src, size_t len);

void mkr_parsed_destroy(mkr_parsed_t *p);

/* ---- attribute -> owner element index (lexbor_compat/attr_owner.c) ----
 *
 * Lexbor sets neither lxb_dom_attr_t::owner nor attr->node.parent, so an
 * attribute node has no usable back-pointer to its element. We build our own
 * map on demand. The XPath attribute axis and Attribute#parent route through
 * this instead of dereferencing a (never-set) owner field.
 */

/* Resolve the element that owns +attr+ within +p+'s document. The index is
 * built lazily on the first call and cached on p->attr_owner. Returns NULL if
 * attr is not in the document, or on allocation failure (fail-closed: a failed
 * build leaves the cache empty so a later call retries). */
lxb_dom_node_t *mkr_parsed_attr_owner(mkr_parsed_t *p, lxb_dom_attr_t *attr);

/* Force the attr->owner index to be built now (idempotent). As a side effect
 * every attribute node's parent pointer is backfilled to its owner element,
 * which the native XPath engine relies on. Returns 0 on success, -1 on
 * allocation failure. Call before evaluating XPath over the document. */
int mkr_parsed_attr_index_build(mkr_parsed_t *p);

/* Drop the attr->owner index so the next lookup rebuilds it. Call after any
 * mutation that can change attribute ownership (attribute set/remove, or
 * inserting a subtree carrying its own attributes). */
void mkr_parsed_attr_index_invalidate(mkr_parsed_t *p);

/* ---- source location (lexbor_compat/source_loc.c) ----
 *
 * mkr_parse_html drives Lexbor's low-level parser pipeline and chains the
 * tokenizer's token-done callback so we can record the byte offset of every
 * element start-tag (always on; the overhead is negligible). After the tree
 * is built we walk it
 * and stamp each element node's offset into node->user (offset + 1; 0 means
 * "unknown"). A line table built from the same input turns that offset into a
 * 1-based line number for Node#line.
 */

/* Line table: 1-based source line for a byte offset. Opaque; stored on
 * mkr_parsed_t.newline_idx. Returns NULL on OOM (callers then report no line). */
void *mkr_lines_build(const lxb_char_t *src, size_t len);
void  mkr_lines_free(void *lines);

/* Token-position recorder, installed as the tokenizer callback (chaining to
 * the parser's tree builder) for the duration of a parse. */
typedef struct mkr_pos_recorder_s mkr_pos_recorder_t;

mkr_pos_recorder_t *mkr_pos_recorder_create(const lxb_char_t *src);
void mkr_pos_recorder_destroy(mkr_pos_recorder_t *rec);
/* Record the parser's own (callback, ctx) so mkr_pos_token_cb can delegate. */
void mkr_pos_recorder_set_delegate(mkr_pos_recorder_t *rec,
                                   lxb_html_tokenizer_token_f orig, void *orig_ctx);
lxb_html_token_t *mkr_pos_token_cb(lxb_html_tokenizer_t *tkz,
                                   lxb_html_token_t *token, void *ctx);
/* Stamp element nodes under root with their recorded byte offsets. */
void mkr_pos_assign_to_dom(mkr_pos_recorder_t *rec, lxb_dom_node_t *root);

/* 1-based source line of node, or 0 if unknown (no tracking / unmatched). */
size_t mkr_parsed_node_line(mkr_parsed_t *p, lxb_dom_node_t *node);

/* Free an index allocated by mkr_parsed_attr_owner. Safe on NULL. Called by
 * mkr_parsed_destroy; exposed so post_parse.c need not see the index layout. */
void mkr_attr_owner_free(void *idx);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_COMPAT_H */
