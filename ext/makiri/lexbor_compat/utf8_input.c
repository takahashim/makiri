#include "compat.h"

#include <lexbor/encoding/encoding.h>

#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/* UTF-8 input sanitisation (browser-compatible HTML decoding)        */
/* ------------------------------------------------------------------ */
/*
 * A self-contained pre-parse subsystem: it turns arbitrary bytes into the
 * valid-UTF-8 the rest of the engine assumes, depending only on Lexbor's
 * encoding API (no parse state). Used by the document parse driver
 * (post_parse.c) and the fragment paths (glue/ruby_doc.c, glue/ruby_mutate.c)
 * via mkr_utf8_sanitize (declared in compat.h).
 */

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
