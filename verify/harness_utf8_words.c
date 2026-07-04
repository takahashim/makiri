/* CBMC harness: the validator's word-to-word transition - the one control
 * flow edge a 12-byte input cannot reach (mkr_utf8_valid consumes 8-byte
 * words while the input stays ASCII, so a SECOND word iteration needs 16
 * bytes with an all-ASCII first word).
 *
 * Proving all 16 arbitrary bytes does not converge (the cost is the chained
 * multibyte decode space, not the word loop - measured >6 min). But the
 * missing transition is only reachable when the first word is ASCII, so
 * assuming that is not a coverage compromise - it is exactly the input region
 * where the transition lives, and it collapses the decode space back to the
 * already-tractable 8-arbitrary-bytes scale. The complement (a non-ASCII
 * byte in the first word) exits the word loop on iteration one, whose control
 * flow harness_utf8_chain.c already proves.
 *
 * With this, every control-flow edge of the validator is covered: 0 and 1
 * word iterations plus word-to-tail (harness_utf8_chain.c), and the
 * word-to-word induction step plus a non-first-iteration word-check failure
 * (here). No new control flow exists at any longer length.
 */
#include "verify.h"
#include "core/mkr_utf8.h"

#include <stdbool.h>

#define N 16

int
main(void)
{
    unsigned char buf[N];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    /* The partition: first word all-ASCII (see the header). */
    for (size_t i = 0; i < 8; ++i) VERIFY_ASSUME(buf[i] < 0x80);

    bool ok = mkr_utf8_valid(buf, len);

    size_t off = 0, steps = 0;
    for (size_t i = 0; i < N && off < len; ++i) {
        uint32_t cp = 0;
        int n = mkr_utf8_decode1(buf + off, len - off, &cp);
        if (n <= 0) break;
        off += (size_t)n;
        steps++;
    }
    VERIFY_ASSERT(ok == (off == len),
                  "validator == chained decoder across a word-to-word transition");
    if (ok)
        VERIFY_ASSERT(mkr_utf8_count_chars((const char *)buf, len) == steps,
                      "count_chars: one per decoded code point");
    return 0;
}
