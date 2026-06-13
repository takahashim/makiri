/* mkr_utf8.c - the shared pure-C UTF-8 validator. Ruby-free, allocation-free.
 * See mkr_utf8.h for the contract and why it lives in core. Moved verbatim from
 * dom_adapter/utf8_input.c (whose sanitiser fast path now calls this). */
#include "mkr_utf8.h"

#include <string.h>   /* memcpy for the word-at-a-time ASCII scan */

bool
mkr_utf8_valid(const unsigned char *src, size_t len)
{
    const unsigned char *p   = src;
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

int
mkr_utf8_decode1(const unsigned char *p, size_t len, uint32_t *cp)
{
    if (len == 0) return 0;
    unsigned char b0 = p[0];
    if (b0 < 0x80u) { *cp = b0; return 1; }

    int n;
    uint32_t c, min;
    if      ((b0 & 0xE0u) == 0xC0u) { n = 2; c = b0 & 0x1Fu; min = 0x80u; }
    else if ((b0 & 0xF0u) == 0xE0u) { n = 3; c = b0 & 0x0Fu; min = 0x800u; }
    else if ((b0 & 0xF8u) == 0xF0u) { n = 4; c = b0 & 0x07u; min = 0x10000u; }
    else return 0;                              /* continuation / 0xF8+ lead */

    if ((size_t)n > len) return 0;              /* truncated */
    for (int i = 1; i < n; i++) {
        unsigned char b = p[i];
        if ((b & 0xC0u) != 0x80u) return 0;     /* bad continuation byte */
        c = (c << 6) | (b & 0x3Fu);
    }
    if (c < min) return 0;                      /* overlong */
    if (c >= 0xD800u && c <= 0xDFFFu) return 0; /* surrogate */
    if (c > 0x10FFFFu) return 0;                /* out of Unicode range */
    *cp = c;
    return n;
}
