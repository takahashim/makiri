/* CBMC harness: whole-buffer agreement between the UTF-8 validator and the
 * strict one-codepoint decoder (core/mkr_utf8), over every byte sequence of
 * up to VERIFY_UTF8_CHAIN_MAX bytes.
 *
 * harness_utf8.c proves the two agree on the FIRST code point; this closes
 * the loop over the whole buffer:
 *   - validator accepts  ==> chaining the decoder consumes exactly len bytes,
 *     and mkr_utf8_count_chars equals the number of decoded code points;
 *   - the chain consumes len bytes ==> the validator accepts (so neither side
 *     is stricter than the other);
 *   - mkr_utf8_advance_chars on valid input always lands on a code-point
 *     boundary (decoding resumes there or the input is exhausted).
 */
#include "verify.h"
#include "core/mkr_utf8.h"

#include <stdbool.h>

#ifndef VERIFY_UTF8_CHAIN_MAX
#define VERIFY_UTF8_CHAIN_MAX 8   /* two full 4-byte code points; ~20 s to prove */
#endif

int
main(void)
{
    unsigned char buf[VERIFY_UTF8_CHAIN_MAX];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    bool ok = mkr_utf8_valid(buf, len);

    /* Chain the strict decoder; each accepted code point is 1..4 bytes, so at
     * most len steps. */
    size_t off = 0, steps = 0;
    for (size_t i = 0; i < VERIFY_UTF8_CHAIN_MAX && off < len; ++i) {
        uint32_t cp = 0;
        int n = mkr_utf8_decode1(buf + off, len - off, &cp);
        if (n <= 0) break;
        off += (size_t)n;
        steps++;
    }
    bool chain_ok = (off == len);

    VERIFY_ASSERT(ok == chain_ok, "validator and chained decoder accept the same buffers");
    if (ok) {
        VERIFY_ASSERT(mkr_utf8_count_chars((const char *)buf, len) == steps,
                      "count_chars: one per decoded code point");

        size_t k = nondet_size_t();
        size_t adv = mkr_utf8_advance_chars((const char *)buf, len, k);
        VERIFY_ASSERT(adv <= len, "advance_chars: clamped");
        if (adv < len) {
            uint32_t cp = 0;
            VERIFY_ASSERT(mkr_utf8_decode1(buf + adv, len - adv, &cp) > 0,
                          "advance_chars: lands on a code-point boundary");
        }
    }
    return 0;
}
