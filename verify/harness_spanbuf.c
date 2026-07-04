/* CBMC harness: the bounded writer (mkr_spanbuf_t, core/mkr_buf.h) checked
 * against a shadow reference model over a nondet op sequence.
 *
 * The spanbuf's contract is "bounds safety by construction": a write that
 * would pass cap is refused, ok latches false and stays false, and a refused
 * write must not advance pos or touch the buffer. The harness replays each op
 * against a shadow copy + canary bytes, so the proof covers
 *   - pos <= cap after every op;
 *   - ok is sticky and pos freezes once latched;
 *   - accepted bytes land exactly (content equality with the shadow);
 *   - bytes past pos are NEVER written (canary survives);
 *   - finish() returns the buffer iff every write was accepted, and the
 *     NULL-backed writer refuses everything.
 */
#include "verify.h"
#include "core/mkr_buf.h"

#include <string.h>

#define BACKING 6
#define OPS     4

int
main(void)
{
    char store[BACKING];
    memset(store, (char)0xAA, sizeof store); /* canary */
    size_t cap = nondet_size_t();
    VERIFY_ASSUME(cap <= BACKING);

    mkr_spanbuf_t f = mkr_spanbuf(store, cap);

    char   shadow[BACKING];
    size_t epos = 0;
    int    eok  = 1;

    for (int step = 0; step < OPS; ++step) {
        if (nondet_uchar() & 1) {
            char c = (char)nondet_uchar();
            mkr_spanbuf_putc(&f, c);
            if (eok && epos < cap) shadow[epos++] = c;
            else                   eok = 0;
        } else {
            unsigned char src[3];
            for (size_t i = 0; i < sizeof src; ++i) src[i] = nondet_uchar();
            size_t n = nondet_size_t();
            VERIFY_ASSUME(n <= sizeof src);
            mkr_spanbuf_write(&f, src, n);
            if (n > 0) {
                if (eok && n <= cap - epos) { memcpy(shadow + epos, src, n); epos += n; }
                else                        eok = 0;
            }
        }
        VERIFY_ASSERT(f.pos <= f.cap, "spanbuf: pos never passes cap");
        VERIFY_ASSERT(f.pos == epos, "spanbuf: refused writes never advance pos");
        VERIFY_ASSERT(f.ok == (eok != 0), "spanbuf: ok latches exactly on the first refusal");
    }

    VERIFY_ASSERT(epos == 0 || memcmp(store, shadow, epos) == 0,
                  "spanbuf: accepted bytes land exactly");
    for (size_t i = epos; i < sizeof store; ++i)
        VERIFY_ASSERT(store[i] == (char)0xAA, "spanbuf: bytes past pos never written");

    const char *out = mkr_spanbuf_finish(&f);
    VERIFY_ASSERT(eok ? (out == store) : (out == NULL),
                  "spanbuf: finish returns the buffer iff all writes were accepted");

    /* mkr_spanbuf_write with src == NULL over a VALID backing buffer: latches
     * not-ok and writes nothing (n > 0), and is a no-op on a fresh writer when
     * n == 0 (the n == 0 short-circuit runs before the NULL check). */
    {
        char store2[4];
        memset(store2, 0x55, sizeof store2);
        mkr_spanbuf_t w = mkr_spanbuf(store2, sizeof store2);
        size_t n = nondet_size_t();
        VERIFY_ASSUME(n >= 1 && n <= sizeof store2);
        mkr_spanbuf_write(&w, NULL, n); /* NULL src, n>0 */
        VERIFY_ASSERT(!w.ok, "spanbuf write NULL n>0: latches not-ok");
        VERIFY_ASSERT(w.pos == 0, "spanbuf write NULL n>0: pos unchanged");
        for (size_t i = 0; i < sizeof store2; ++i)
            VERIFY_ASSERT(store2[i] == (char)0x55, "spanbuf write NULL n>0: buffer untouched");

        /* NULL src, n==0 is a no-op on a FRESH ok writer. */
        mkr_spanbuf_t w0 = mkr_spanbuf(store2, sizeof store2);
        mkr_spanbuf_write(&w0, NULL, 0);
        VERIFY_ASSERT(w0.ok && w0.pos == 0, "spanbuf write NULL n==0: no-op");
    }

    /* NULL backing: permanently not-ok, every write a safe no-op. */
    {
        mkr_spanbuf_t g = mkr_spanbuf(NULL, nondet_size_t());
        mkr_spanbuf_putc(&g, 'x');
        mkr_spanbuf_write(&g, "yz", 2);
        VERIFY_ASSERT(!g.ok && g.pos == 0 && mkr_spanbuf_finish(&g) == NULL,
                      "spanbuf: NULL backing refuses everything");
    }
    return 0;
}
