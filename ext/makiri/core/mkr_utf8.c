/* mkr_utf8.c - the shared pure-C UTF-8 validator. Ruby-free, allocation-free.
 * See mkr_utf8.h for the contract and why it lives in core. Moved verbatim from
 * lexbor_compat/utf8_input.c (whose sanitiser fast path now calls this). */
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
