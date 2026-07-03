/* CBMC harness: the strict one-codepoint UTF-8 decoder + the validator
 * (core/mkr_utf8). The decoder's input unit is at most 4 bytes, so a nondet
 * 4-byte buffer makes this an exhaustive check, not a sample.
 *
 * Properties:
 *   - decode1 never reads past [p, p+len) (CBMC --bounds-check does the work);
 *   - on success the length is 1..4 and within len, the code point is a
 *     Unicode scalar value (<= U+10FFFF, not a surrogate), and the consumed
 *     prefix is well-formed per the validator;
 *   - decoder and validator agree: a validator-accepted non-empty buffer
 *     always decodes.
 */
#include "verify.h"
#include "core/mkr_utf8.h"

#include <stdbool.h>

int
main(void)
{
    /* --- decoder: exhaustive over its 4-byte input unit --- */
    {
        unsigned char buf[4];
        for (size_t i = 0; i < sizeof buf; ++i) buf[i] = nondet_uchar();
        size_t len = nondet_size_t();
        VERIFY_ASSUME(len <= sizeof buf);

        uint32_t cp = 0;
        int n = mkr_utf8_decode1(buf, len, &cp);
        if (n > 0) {
            VERIFY_ASSERT(n >= 1 && n <= 4 && (size_t)n <= len, "decode1: length in bounds");
            VERIFY_ASSERT(cp <= 0x10FFFF, "decode1: scalar range");
            VERIFY_ASSERT(!(cp >= 0xD800 && cp <= 0xDFFF), "decode1: no surrogates");
            bool prefix_ok = mkr_utf8_valid(buf, (size_t)n);
            VERIFY_ASSERT(prefix_ok, "decode1: accepted prefix is well-formed");
        } else {
            VERIFY_ASSERT(n == 0, "decode1: only 0 signals failure");
        }
    }

    /* --- validator: memory safety + agreement with the decoder --- */
    {
        unsigned char buf[8];
        for (size_t i = 0; i < sizeof buf; ++i) buf[i] = nondet_uchar();
        size_t len = nondet_size_t();
        VERIFY_ASSUME(len <= sizeof buf);

        bool ok = mkr_utf8_valid(buf, len);
        if (ok && len > 0) {
            uint32_t cp = 0;
            int n = mkr_utf8_decode1(buf, len, &cp);
            VERIFY_ASSERT(n > 0, "valid non-empty input decodes");
        }
    }

    /* --- char counting/advancing: bounded, never past len --- */
    {
        unsigned char buf[6];
        for (size_t i = 0; i < sizeof buf; ++i) buf[i] = nondet_uchar();
        size_t len = nondet_size_t(), nchars = nondet_size_t();
        VERIFY_ASSUME(len <= sizeof buf);

        size_t cnt = mkr_utf8_count_chars((const char *)buf, len);
        VERIFY_ASSERT(cnt <= len, "count_chars: at most one per byte");

        size_t off = mkr_utf8_advance_chars((const char *)buf, len, nchars);
        VERIFY_ASSERT(off <= len, "advance_chars: clamped at len");
    }
    return 0;
}
