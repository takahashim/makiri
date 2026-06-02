#include "compat.h"
#include "../core/mkr_safe.h"

#include <lexbor/html/parser.h>
#include <lexbor/html/tokenizer.h>

#include <stdlib.h>

/* UTF-8 input sanitisation (mkr_utf8_sanitize) is a separate pre-parse
 * subsystem in utf8_input.c; this file drives the parse pipeline and owns the
 * parsed-document lifecycle. */

/* Source-location path: drive the low-level parser pipeline so we can chain
 * the tokenizer callback and capture element offsets, then build the line
 * table. Returns the document on success, NULL on failure (everything it
 * allocated is released). On success *out_lines receives the line table
 * (possibly NULL if that allocation failed — line info then degrades to nil). */
static lxb_html_document_t *
mkr_parse_tracked(const lxb_char_t *src, size_t len, void **out_lines)
{
    *out_lines = NULL;

    lxb_html_parser_t *parser = lxb_html_parser_create();
    if (parser == NULL || lxb_html_parser_init(parser) != LXB_STATUS_OK) {
        if (parser != NULL) {
            lxb_html_parser_destroy(parser);
        }
        return NULL;
    }

    lxb_html_document_t *doc = lxb_html_parse_chunk_begin(parser);
    if (doc == NULL) {
        lxb_html_parser_destroy(parser);
        return NULL;
    }

    /* Install the recorder, chaining the parser's own tree-building callback
     * (set by chunk_begin). If the recorder can't be allocated we simply parse
     * without source tracking. */
    mkr_pos_recorder_t *rec = mkr_pos_recorder_create(src);
    if (rec != NULL) {
        lxb_html_tokenizer_t *tkz = lxb_html_parser_tokenizer(parser);
        mkr_pos_recorder_set_delegate(rec, tkz->callback_token_done,
                                      tkz->callback_token_ctx);
        lxb_html_tokenizer_callback_token_done_set(tkz, mkr_pos_token_cb, rec);
    }

    lxb_status_t st = lxb_html_parse_chunk_process(parser, src, len);
    if (st == LXB_STATUS_OK) {
        st = lxb_html_parse_chunk_end(parser);
    }

    if (st != LXB_STATUS_OK) {
        mkr_pos_recorder_destroy(rec);
        lxb_html_document_destroy(doc);
        lxb_html_parser_destroy(parser);
        return NULL;
    }

    if (rec != NULL) {
        mkr_pos_assign_to_dom(rec, lxb_dom_interface_node(doc));
        mkr_pos_recorder_destroy(rec);
        *out_lines = mkr_lines_build(src, len);
    }

    /* The document outlives the parser (parser_destroy only unrefs the
     * tokenizer and tree, never the document). */
    lxb_html_parser_destroy(parser);
    return doc;
}

mkr_parsed_t *
mkr_parse_html(const lxb_char_t *src, size_t len)
{
    if (src == NULL && len != 0) {
        return NULL;
    }

    mkr_parsed_t *p = mkr_callocarray(1, sizeof(*p));
    if (p == NULL) {
        return NULL;
    }

    /* Empty input (NULL source): nothing to sanitise or track — just create an
     * empty document. */
    if (src == NULL) {
        p->doc = lxb_html_document_create();
        if (p->doc != NULL
            && lxb_html_document_parse(p->doc, src, len) != LXB_STATUS_OK) {
            lxb_html_document_destroy(p->doc);
            p->doc = NULL;
        }
        if (p->doc == NULL) {
            free(p);
            return NULL;
        }
        return p;
    }

    /* Browser-compatible decoding: invalid UTF-8 is replaced with U+FFFD
     * (WHATWG byte-stream decoding) so parsing never fails on bad bytes and the
     * DOM is always valid UTF-8. Valid input (the common case) is used as-is
     * with no copy. Source offsets / line numbers are then relative to the
     * sanitised bytes — exact for valid input, best-effort when replacement
     * shifted byte positions. Source locations always ride the parse pipeline
     * (cheap). */
    lxb_char_t *clean = NULL;
    size_t      clean_len = 0;
    if (mkr_utf8_sanitize(src, len, &clean, &clean_len) != 0) {
        free(p);
        return NULL; /* OOM */
    }
    p->doc = mkr_parse_tracked((clean != NULL) ? clean : src,
                               (clean != NULL) ? clean_len : len,
                               &p->newline_idx);
    free(clean); /* safe on NULL */
    if (p->doc == NULL) {
        free(p);
        return NULL;
    }
    return p;
}

void
mkr_parsed_destroy(mkr_parsed_t *p)
{
    if (p == NULL) {
        return;
    }

    /* attr_owner is built lazily (M4) and is NULL if no attribute lookup ever
     * ran; mkr_attr_owner_free is a no-op on NULL. newline_idx (M6) is set only
     * when source-location tracking ran; mkr_lines_free is a no-op on NULL. */
    mkr_attr_owner_free(p->attr_owner);
    p->attr_owner = NULL;
    mkr_lines_free(p->newline_idx);
    p->newline_idx = NULL;
    mkr_text_index_free(p->text_index); /* lazy; NULL if no text query ran */
    p->text_index = NULL;

    if (p->doc != NULL) {
        lxb_html_document_destroy(p->doc);
        p->doc = NULL;
    }

    free(p);
}
