#include "compat.h"
#include "../core/mkr_core.h"

#include <lexbor/encoding/encoding.h>

#include <string.h>   /* memcpy for the word-at-a-time ASCII scan */

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

/* UTF-8 validation: the shared, allocation-free validator lives in core
 * (core/mkr_utf8.c, via mkr_core.h) so this fast path and the Ruby bridge's
 * strict input gate (mkr_verify_text) run ONE implementation. Its contract
 * here is unchanged: it returns true *only* for input that
 * mkr_utf8_replace_invalid would leave byte-identical, so "valid" can safely
 * skip the transcode. (lxb_char_t is unsigned char, so the call is direct.) */

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
    mkr_buf_t buf;

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

    /* The output is at most 3x the input: WHATWG byte-stream decoding replaces
     * each invalid byte with U+FFFD (3 bytes) and passes valid bytes through 1:1.
     * Cap the buffer at exactly that bound - tight and tied to the actual input,
     * so a large document still parses but nothing runs away - rather than a
     * blanket ceiling. (len > 0 here: mkr_utf8_sanitize returns early on len == 0.)
     * It grows overflow-safe and fails closed (NULL) on OOM or the cap. */
    size_t cap;
    if (!mkr_size_mul(len, 3, &cap)) {
        cap = MKR_BUF_HARD_MAX;   /* unreachable for any real in-memory string */
    }
    mkr_buf_init(&buf, cap);

#define MKR_FLUSH()                                                            \
    do {                                                                       \
        if (mkr_buf_append(&buf, outbuf,                                       \
                           lxb_encoding_encode_buf_used(&enc)) != MKR_OK) {    \
            mkr_buf_free(&buf);                                                \
            return NULL;                                                       \
        }                                                                      \
        lxb_encoding_encode_buf_used_set(&enc, 0);                             \
    } while (0)

    do {
        de_status = u8->decode(&dec, &data, end);
        cp_begin = cp;
        cp_end = cp_begin + lxb_encoding_decode_buf_used(&dec);
        do {
            en_status = u8->encode(&enc, &cp_begin, cp_end);
            MKR_FLUSH();
        } while (en_status == LXB_STATUS_SMALL_BUFFER);
        lxb_encoding_decode_buf_used_set(&dec, 0);
    } while (de_status == LXB_STATUS_SMALL_BUFFER);

    /* Flush an incomplete trailing sequence as one U+FFFD. */
    (void) lxb_encoding_decode_finish(&dec);
    if (lxb_encoding_decode_buf_used(&dec) != 0) {
        cp_begin = cp;
        cp_end = cp_begin + lxb_encoding_decode_buf_used(&dec);
        (void) u8->encode(&enc, &cp_begin, cp_end);
        MKR_FLUSH();
    }
    (void) lxb_encoding_encode_finish(&enc);
    MKR_FLUSH();

#undef MKR_FLUSH

    return (lxb_char_t *) mkr_buf_steal(&buf, out_len);
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
