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

/* Is `src` (len bytes) well-formed UTF-8? A dedicated validator (the Unicode
 * "well-formed UTF-8 byte sequences" table, RFC 3629 / WHATWG): it rejects bad
 * continuation bytes, overlong forms, surrogates (U+D800..U+DFFF) and code
 * points above U+10FFFF, and an incomplete trailing sequence. This is the same
 * accept set as Lexbor's decoder but validate-only - it never materialises code
 * points, and rips through ASCII (the common case) a machine word at a time, so
 * it is much cheaper than decode-and-discard. NUL bytes are valid here and are
 * left for the HTML tokenizer to handle per the spec.
 *
 * The contract that matters: this returns true *only* for input that
 * mkr_utf8_replace_invalid would leave byte-identical, so "valid" can safely
 * skip the transcode. */
static bool
mkr_utf8_valid(const lxb_char_t *src, size_t len)
{
    const unsigned char *p   = (const unsigned char *)src;
    const unsigned char *const end = p + len;

    while (p < end) {
        unsigned char b = *p;

        if (b < 0x80) {
            /* ASCII fast path: skip a run of ASCII bytes a word at a time
             * (any high bit set ends the run), then byte-wise for the tail. */
            while ((size_t)(end - p) >= sizeof(size_t)) {
                size_t w;
                memcpy(&w, p, sizeof(w));
                if (w & (size_t)0x8080808080808080ULL) {
                    break;
                }
                p += sizeof(size_t);
            }
            while (p < end && *p < 0x80) {
                p++;
            }
            continue;
        }

        /* Multi-byte: decide length and validate the (length-dependent) ranges
         * that exclude overlong forms, surrogates and > U+10FFFF. */
        size_t n;
        if (b >= 0xC2 && b <= 0xDF) {                 /* U+0080..U+07FF   */
            n = 2;
            if (end - p < 2 || (p[1] & 0xC0) != 0x80) return false;
        } else if (b == 0xE0) {                       /* U+0800..U+0FFF   */
            n = 3;
            if (end - p < 3 || p[1] < 0xA0 || p[1] > 0xBF
                || (p[2] & 0xC0) != 0x80) return false;
        } else if (b >= 0xE1 && b <= 0xEC) {          /* U+1000..U+CFFF   */
            n = 3;
            if (end - p < 3 || (p[1] & 0xC0) != 0x80
                || (p[2] & 0xC0) != 0x80) return false;
        } else if (b == 0xED) {                       /* U+D000..U+D7FF   */
            n = 3;                                    /* (excludes surrogates) */
            if (end - p < 3 || p[1] < 0x80 || p[1] > 0x9F
                || (p[2] & 0xC0) != 0x80) return false;
        } else if (b == 0xEE || b == 0xEF) {          /* U+E000..U+FFFF   */
            n = 3;
            if (end - p < 3 || (p[1] & 0xC0) != 0x80
                || (p[2] & 0xC0) != 0x80) return false;
        } else if (b == 0xF0) {                       /* U+10000..U+3FFFF */
            n = 4;
            if (end - p < 4 || p[1] < 0x90 || p[1] > 0xBF
                || (p[2] & 0xC0) != 0x80 || (p[3] & 0xC0) != 0x80) return false;
        } else if (b >= 0xF1 && b <= 0xF3) {          /* U+40000..U+FFFFF */
            n = 4;
            if (end - p < 4 || (p[1] & 0xC0) != 0x80 || (p[2] & 0xC0) != 0x80
                || (p[3] & 0xC0) != 0x80) return false;
        } else if (b == 0xF4) {                       /* U+100000..U+10FFFF */
            n = 4;
            if (end - p < 4 || p[1] < 0x80 || p[1] > 0x8F
                || (p[2] & 0xC0) != 0x80 || (p[3] & 0xC0) != 0x80) return false;
        } else {                                      /* C0,C1,F5..FF,stray 80..BF */
            return false;
        }
        p += n;
    }
    return true;
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
