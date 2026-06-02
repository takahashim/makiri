#ifndef MAKIRI_COMPAT_H
#define MAKIRI_COMPAT_H

#include <stdbool.h>
#include <stddef.h>

#include <lexbor/html/html.h>
#include <lexbor/dom/dom.h>

#include "../core/mkr_safe.h" /* mkr_borrowed_text_t */

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
    void                *text_index;   /* node -> descendant-text slice run */
} mkr_parsed_t;

/* Parse HTML5 from src[0..len). Returns NULL on allocation or parse failure
 * (fail-closed). Source locations for Node#line are always tracked via the
 * tokenizer (cheap — it rides the parse). The src buffer is not retained:
 * Lexbor copies what it needs into the arena, and we build the line table up
 * front. */
mkr_parsed_t *mkr_parse_html(const lxb_char_t *src, size_t len);

/* Browser-compatible UTF-8 input sanitisation. If `src` (len bytes) is already
 * well-formed UTF-8 (the common case), sets *out=NULL and the caller uses src
 * unchanged. Otherwise transcodes to a freshly malloc'd, NUL-terminated buffer
 * with every invalid sequence replaced by U+FFFD (WHATWG decoding), setting
 * *out / *out_len (caller frees *out). Returns 0 on success, -1 on OOM. NUL
 * bytes are preserved (the HTML tokenizer handles them per the spec). Used by
 * the parse and fragment-parse paths; never rejects. */
int mkr_utf8_sanitize(const lxb_char_t *src, size_t len,
                      lxb_char_t **out, size_t *out_len);

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
 * inserting a subtree carrying its own attributes). This also drops the
 * co-built element index below. */
void mkr_parsed_attr_index_invalidate(mkr_parsed_t *p);

/* ---- element index: tag id -> elements (lexbor_compat/attr_owner.c) ----
 *
 * Co-built with the attr->owner index in the same document walk (same object,
 * same lazy build, same invalidation). Groups every element by tag id in
 * document order so the XPath engine can answer `//tag` (a descendant
 * name-test rooted at the document) without walking the tree.
 */

/* The built element index for +p+ (lazily built; NULL on allocation failure).
 * The returned pointer is the opaque index passed to the lookups below. */
void *mkr_parsed_element_index(mkr_parsed_t *p);

/* Borrowed, document-ordered array of every element whose tag id == +tag_id+,
 * with the count via *count. NULL / *count == 0 when there are none. Valid
 * until the index is invalidated. */
lxb_dom_node_t *const *mkr_element_index_tag(const void *idx, lxb_tag_id_t tag_id,
                                             size_t *count);

/* Nonzero if the document contains any non-HTML-namespace element. The tag
 * fast path is only sound for pure-HTML documents (it assumes an element's
 * qualified name equals its tag's canonical name), so the engine must consult
 * this and fall back to the tree walk when it is nonzero. Returns nonzero
 * (fail safe) for a NULL index. */
int mkr_element_index_has_foreign(const void *idx);

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

/* ---- text-extraction index (lexbor_compat/text_index.c) ----
 *
 * Maps a node to the contiguous run of document-order TEXT/CDATA byte slices
 * its subtree owns, so Node#text / XPath string-value can serve a pre-sized
 * single memcpy run instead of walking the subtree. Built lazily and cached on
 * mkr_parsed_t.text_index; dropped by mkr_parsed_text_index_invalidate from the
 * same mutation hook as the attr index (a cached slice borrows into a text
 * node's storage, valid only until a mutation reallocates/detaches it).
 */

/* If +node+ is in +p+'s indexed document tree, set *slices and *n to its
 * document-order descendant-text slice run (each a borrowed view of one
 * text/CDATA node's data) and *total_bytes to their summed length, and return
 * 1. Returns 0 (caller falls back to a tree walk) when node is outside the
 * indexed tree (e.g. a fragment) or the index cannot be built (OOM). Builds the
 * index on first use. */
int mkr_parsed_text_slices(mkr_parsed_t *p, const lxb_dom_node_t *node,
                           const mkr_borrowed_text_t **slices, size_t *n,
                           size_t *total_bytes);

/* Drop the text index so the next query rebuilds it. Call after any mutation. */
void mkr_parsed_text_index_invalidate(mkr_parsed_t *p);

/* Free a text index. Safe on NULL. Called by mkr_parsed_destroy. */
void mkr_text_index_free(void *idx);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_COMPAT_H */
