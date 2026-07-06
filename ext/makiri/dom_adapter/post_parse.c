#include "compat.h"
#include "../core/mkr_core.h"
#include "../xml/mkr_xml.h"   /* mkr_xml_doc_destroy for the XML destroy branch */

#include <lexbor/html/parser.h>
#include <lexbor/html/tokenizer.h>

#include <stdlib.h>
#include <assert.h>

/* UTF-8 input sanitisation (mkr_utf8_sanitize) is a separate pre-parse
 * subsystem in utf8_input.c; this file drives the parse pipeline and owns the
 * parsed-document lifecycle. */

/* Source-location path: drive the low-level parser pipeline so we can chain
 * the tokenizer callback and capture element offsets, then build the line
 * table. Returns the document on success, NULL on failure (everything it
 * allocated is released). On success *out_lines receives the line table
 * (possibly NULL if that allocation failed - line info then degrades to nil). */
static lxb_html_document_t *
mkr_parse_tracked(const lxb_char_t *src, size_t len, void **out_lines)
{
    *out_lines = NULL;

    /* All resources declared up front and freed once through `fail:` (single-exit),
     * so the correct free-set lives in one place instead of being re-spelled at
     * each early return. Every destructor below is NULL-safe (guarded / by
     * contract), so ordering across the acquisition points stays trivial. */
    lxb_html_parser_t   *parser = NULL;
    lxb_html_document_t *doc    = NULL;
    mkr_pos_recorder_t  *rec    = NULL;
    lxb_status_t         st;

    parser = lxb_html_parser_create();
    if (parser == NULL || lxb_html_parser_init(parser) != LXB_STATUS_OK) goto fail;

    doc = lxb_html_parse_chunk_begin(parser);
    if (doc == NULL) goto fail;

    /* Install the recorder, chaining the parser's own tree-building callback
     * (set by chunk_begin). If the recorder can't be allocated we simply parse
     * without source tracking. */
    rec = mkr_pos_recorder_create(src);
    if (rec != NULL) {
        lxb_html_tokenizer_t *tkz = lxb_html_parser_tokenizer(parser);
        /* Lexbor has a setter and a ctx getter for the token-done callback but no
         * getter for the callback function itself, so read that one field
         * directly (the ctx uses the public getter). */
        mkr_pos_recorder_set_delegate(rec, tkz->callback_token_done,
                                      lxb_html_tokenizer_callback_token_done_ctx(tkz));
        lxb_html_tokenizer_callback_token_done_set(tkz, mkr_pos_token_cb, rec);
    }

    st = lxb_html_parse_chunk_process(parser, src, len);
    if (st == LXB_STATUS_OK) {
        st = lxb_html_parse_chunk_end(parser);
    }
    if (st != LXB_STATUS_OK) goto fail;

    if (rec != NULL) {
        mkr_pos_assign_to_dom(rec, lxb_dom_interface_node(doc));
        mkr_pos_recorder_destroy(rec);
        rec = NULL;                    /* consumed */
        *out_lines = mkr_lines_build(src, len);
    }

    /* The document outlives the parser (parser_destroy only unrefs the
     * tokenizer and tree, never the document). */
    lxb_html_parser_destroy(parser);
    return doc;

fail:
    mkr_pos_recorder_destroy(rec);                  /* NULL-safe */
    if (doc != NULL)    lxb_html_document_destroy(doc);
    if (parser != NULL) lxb_html_parser_destroy(parser);
    return NULL;
}

mkr_parsed_t *
mkr_parse_html(const lxb_char_t *src, size_t len, bool assume_valid)
{
    if (src == NULL && len != 0) {
        return NULL;
    }

    mkr_parsed_t *p = mkr_callocarray(1, sizeof(*p));
    if (p == NULL) {
        return NULL;
    }
    p->kind = MKR_DOC_HTML;

    /* Empty input (NULL source): nothing to sanitise or track - just create an
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
     * sanitised bytes - exact for valid input, best-effort when replacement
     * shifted byte positions. Source locations always ride the parse pipeline
     * (cheap). `assume_valid` (the caller already verified UTF-8, e.g. via the
     * Ruby String's coderange) skips the validation scan entirely. */
    lxb_char_t *clean = NULL;
    size_t      clean_len = 0;
    if (!assume_valid && mkr_utf8_sanitize(src, len, &clean, &clean_len) != 0) {
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

    /* The compat indices are HTML-only and built lazily; each *_free is a no-op
     * on NULL, so this is safe for an XML handle (which never sets them). */
    mkr_dom_index_free(p->dom_index);
    p->dom_index = NULL;
    mkr_lines_free(p->newline_idx);
    p->newline_idx = NULL;
    mkr_text_index_free(p->text_index); /* lazy; NULL if no text query ran */
    p->text_index = NULL;

    if (p->doc != NULL) {
        if (p->kind == MKR_DOC_XML) {
            mkr_xml_doc_destroy((mkr_xml_doc_t *)p->doc); /* whole-arena free */
        } else {
            lxb_html_document_destroy((lxb_html_document_t *)p->doc);
        }
        p->doc = NULL;
    }

    free(p);
}

/* ---- document-kind accessors (§2.3) ---- */

mkr_doc_kind_t
mkr_parsed_kind(const mkr_parsed_t *p)
{
    return p->kind;
}

lxb_html_document_t *
mkr_parsed_html_doc(const mkr_parsed_t *p)
{
    assert(p->kind == MKR_DOC_HTML);
    return (lxb_html_document_t *)p->doc;
}

mkr_parsed_t *
mkr_parsed_new_xml(struct mkr_xml_doc *xdoc)
{
    mkr_parsed_t *p = mkr_callocarray(1, sizeof(*p));
    if (p == NULL) {
        return NULL;
    }
    p->kind = MKR_DOC_XML;
    p->doc  = xdoc;            /* may be NULL; set later via mkr_parsed_set_xml_doc */
    return p;
}

struct mkr_xml_doc *
mkr_parsed_xml_doc(const mkr_parsed_t *p)
{
    assert(p->kind == MKR_DOC_XML);
    return (struct mkr_xml_doc *)p->doc;
}

void
mkr_parsed_set_xml_doc(mkr_parsed_t *p, struct mkr_xml_doc *xdoc)
{
    assert(p->kind == MKR_DOC_XML);
    p->doc = xdoc;
}

/* Bytes handed out from one Lexbor mem pool. Lexbor exposes no running total, so
 * walk the chunk list summing each chunk's bump length. Cheap: chunks are few,
 * large blocks. Saturates to SIZE_MAX on the (unreachable) overflow; the caller
 * clamps the derived cap to the buffer hard ceiling anyway. */
static size_t
mkr_lxb_mem_used(const lexbor_mem_t *mem)
{
    size_t total = 0;
    for (const lexbor_mem_chunk_t *c = (mem != NULL) ? mem->chunk_first : NULL;
         c != NULL; c = c->next) {
        if (!mkr_size_add(total, c->length, &total)) {
            return SIZE_MAX;
        }
    }
    return total;
}

size_t
mkr_lxb_document_bytes(lxb_dom_node_t *node)
{
    if (node == NULL) {
        return 0;
    }
    /* The document node owns itself; every other node points back via owner_document. */
    lxb_dom_document_t *doc = (node->type == LXB_DOM_NODE_TYPE_DOCUMENT)
                                  ? lxb_dom_interface_document(node)
                                  : node->owner_document;
    if (doc == NULL) {
        return 0;
    }
    size_t total = 0;
    if (doc->mraw != NULL
        && !mkr_size_add(total, mkr_lxb_mem_used(doc->mraw->mem), &total)) {
        return SIZE_MAX;
    }
    if (doc->text != NULL
        && !mkr_size_add(total, mkr_lxb_mem_used(doc->text->mem), &total)) {
        return SIZE_MAX;
    }
    return total;
}
