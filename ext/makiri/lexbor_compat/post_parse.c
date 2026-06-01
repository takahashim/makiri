#include "compat.h"

#include <lexbor/html/parser.h>
#include <lexbor/html/tokenizer.h>
#include <lexbor/encoding/encoding.h>

#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/* UTF-8 input sanitisation (browser-compatible HTML decoding)        */
/* ------------------------------------------------------------------ */

/* Is `src` (len bytes) well-formed UTF-8? Uses Lexbor's spec UTF-8 decoder
 * (which rejects bad continuation bytes, overlong forms, surrogates and
 * out-of-range code points) with NO replacement, so the first error makes it
 * return false. An incomplete trailing sequence (caught by decode_finish) is
 * also invalid. NUL bytes are valid UTF-8 here and are left for the HTML
 * tokenizer to handle per the spec (drop in text, U+FFFD in foreign content). */
static bool
mkr_utf8_valid(const lxb_char_t *src, size_t len)
{
    const lxb_encoding_data_t *u8 = lxb_encoding_data(LXB_ENCODING_UTF_8);
    lxb_encoding_decode_t dec;
    lxb_codepoint_t cp[1024];
    const lxb_char_t *data = src, *end = src + len;
    lxb_status_t st;

    if (lxb_encoding_decode_init(&dec, u8, cp,
                                 sizeof(cp) / sizeof(cp[0])) != LXB_STATUS_OK) {
        return false; /* treat as invalid -> caller transcodes (fail safe) */
    }
    /* No replace_set: an invalid sequence aborts with LXB_STATUS_ERROR. */
    do {
        st = u8->decode(&dec, &data, end);
        lxb_encoding_decode_buf_used_set(&dec, 0); /* discard code points */
    } while (st == LXB_STATUS_SMALL_BUFFER);

    if (st != LXB_STATUS_OK) {
        return false;
    }
    return lxb_encoding_decode_finish(&dec) == LXB_STATUS_OK;
}

/* Transcode UTF-8 -> UTF-8 replacing every invalid sequence with U+FFFD
 * (WHATWG byte-stream decoding), into a freshly malloc'd, NUL-terminated
 * buffer. Sets *out_len (excluding the terminator). Returns NULL on OOM. */
static lxb_char_t *
mkr_utf8_replace_invalid(const lxb_char_t *src, size_t len, size_t *out_len)
{
    const lxb_encoding_data_t *u8 = lxb_encoding_data(LXB_ENCODING_UTF_8);
    lxb_encoding_decode_t dec;
    lxb_encoding_encode_t enc;
    lxb_codepoint_t cp[1024];
    lxb_char_t outbuf[4096];
    const lxb_char_t *data = src, *end = src + len;
    const lxb_codepoint_t *cp_begin, *cp_end;
    lxb_status_t de_status, en_status;
    size_t cap, n = 0;
    lxb_char_t *result;

    if (lxb_encoding_decode_init(&dec, u8, cp,
                                 sizeof(cp) / sizeof(cp[0])) != LXB_STATUS_OK) {
        return NULL;
    }
    (void) lxb_encoding_decode_replace_set(&dec, LXB_ENCODING_REPLACEMENT_BUFFER,
                                           LXB_ENCODING_REPLACEMENT_BUFFER_LEN);
    if (lxb_encoding_encode_init(&enc, u8, outbuf, sizeof(outbuf)) != LXB_STATUS_OK) {
        return NULL;
    }
    (void) lxb_encoding_encode_replace_set(&enc, LXB_ENCODING_REPLACEMENT_BYTES,
                                           LXB_ENCODING_REPLACEMENT_SIZE);

    cap = len + 16;
    result = malloc(cap + 1);
    if (result == NULL) {
        return NULL;
    }

#define MKR_EMIT(buf, blen)                                                    \
    do {                                                                       \
        size_t _bl = (blen);                                                   \
        if (n + _bl > cap) {                                                   \
            size_t _nc = (cap * 2 > n + _bl) ? cap * 2 : n + _bl;              \
            lxb_char_t *_r = realloc(result, _nc + 1);                         \
            if (_r == NULL) { free(result); return NULL; }                     \
            result = _r; cap = _nc;                                            \
        }                                                                      \
        memcpy(result + n, (buf), _bl);                                        \
        n += _bl;                                                              \
    } while (0)

    do {
        de_status = u8->decode(&dec, &data, end);
        cp_begin = cp;
        cp_end = cp_begin + lxb_encoding_decode_buf_used(&dec);
        do {
            en_status = u8->encode(&enc, &cp_begin, cp_end);
            MKR_EMIT(outbuf, lxb_encoding_encode_buf_used(&enc));
            lxb_encoding_encode_buf_used_set(&enc, 0);
        } while (en_status == LXB_STATUS_SMALL_BUFFER);
        lxb_encoding_decode_buf_used_set(&dec, 0);
    } while (de_status == LXB_STATUS_SMALL_BUFFER);

    /* Flush an incomplete trailing sequence as one U+FFFD. */
    (void) lxb_encoding_decode_finish(&dec);
    if (lxb_encoding_decode_buf_used(&dec) != 0) {
        cp_begin = cp;
        cp_end = cp_begin + lxb_encoding_decode_buf_used(&dec);
        (void) u8->encode(&enc, &cp_begin, cp_end);
        MKR_EMIT(outbuf, lxb_encoding_encode_buf_used(&enc));
        lxb_encoding_encode_buf_used_set(&enc, 0);
    }
    (void) lxb_encoding_encode_finish(&enc);
    MKR_EMIT(outbuf, lxb_encoding_encode_buf_used(&enc));

#undef MKR_EMIT

    result[n] = '\0';
    *out_len = n;
    return result;
}

int
mkr_utf8_sanitize(const lxb_char_t *src, size_t len,
                  lxb_char_t **out, size_t *out_len)
{
    *out = NULL;
    *out_len = 0;
    if (src == NULL || len == 0 || mkr_utf8_valid(src, len)) {
        return 0; /* already valid UTF-8 (the common case): use src as-is */
    }
    *out = mkr_utf8_replace_invalid(src, len, out_len);
    return (*out == NULL) ? -1 : 0;
}

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

    mkr_parsed_t *p = calloc(1, sizeof(*p));
    if (p == NULL) {
        return NULL;
    }

    /* Source locations are always tracked via the tokenizer pipeline (cheap —
     * it rides the parse). The NULL-source edge case (empty input) takes the
     * plain path since there is nothing to scan. */
    if (src != NULL) {
        /* Browser-compatible decoding: invalid UTF-8 is replaced with U+FFFD
         * (WHATWG byte-stream decoding) so parsing never fails on bad bytes and
         * the DOM is always valid UTF-8. Valid input (the common case) is used
         * as-is with no copy. Source offsets / line numbers are then relative
         * to the sanitised bytes — exact for valid input, best-effort when
         * replacement shifted byte positions. */
        lxb_char_t *clean = NULL;
        size_t      clean_len = 0;
        if (mkr_utf8_sanitize(src, len, &clean, &clean_len) != 0) {
            free(p);
            return NULL; /* OOM */
        }
        const lxb_char_t *psrc = (clean != NULL) ? clean : src;
        size_t            plen = (clean != NULL) ? clean_len : len;

        p->doc = mkr_parse_tracked(psrc, plen, &p->newline_idx);
        free(clean); /* safe on NULL */
        if (p->doc == NULL) {
            free(p);
            return NULL;
        }
        return p;
    }

    p->doc = lxb_html_document_create();
    if (p->doc == NULL) {
        free(p);
        return NULL;
    }
    if (lxb_html_document_parse(p->doc, src, len) != LXB_STATUS_OK) {
        lxb_html_document_destroy(p->doc);
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

    if (p->doc != NULL) {
        lxb_html_document_destroy(p->doc);
        p->doc = NULL;
    }

    free(p);
}
