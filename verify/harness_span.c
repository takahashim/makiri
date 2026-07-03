/* CBMC harness: the bounded reader (core/mkr_span.h) and the slice helpers.
 *
 * Property: whatever bounded sequence of operations runs over a span, the
 * cursor invariant base <= p <= end holds and no read leaves the buffer
 * (--bounds-check). The ops are chosen nondeterministically per step, so the
 * check covers every interleaving up to VERIFY_SPAN_OPS steps.
 */
#include "verify.h"
#include "core/mkr_span.h"

#ifndef VERIFY_SPAN_OPS
#define VERIFY_SPAN_OPS 4
#endif

int
main(void)
{
    char buf[8];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    mkr_span_t s = mkr_span(buf, len);
    const char *base = s.p;
    const char *end  = s.end;

    for (int step = 0; step < VERIFY_SPAN_OPS; ++step) {
        unsigned char op = nondet_uchar();
        switch (op % 7) {
        case 0: {
            int c = mkr_span_peek(&s);
            VERIFY_ASSERT(c >= -1 && c <= 255, "peek: byte or -1");
            break;
        }
        case 1: {
            int c = mkr_span_take(&s);
            VERIFY_ASSERT(c >= -1 && c <= 255, "take: byte or -1");
            break;
        }
        case 2:
            mkr_span_skip(&s, nondet_size_t());
            break;
        case 3: {
            int c = mkr_span_at(&s, nondet_size_t());
            VERIFY_ASSERT(c >= -1 && c <= 255, "at: byte or -1");
            break;
        }
        case 4: {
            size_t idx = 0;
            if (mkr_span_find(&s, (char)nondet_uchar(), &idx))
                VERIFY_ASSERT(idx < mkr_span_left(&s), "find: hit inside remainder");
            break;
        }
        case 5: {
            mkr_span_t t = mkr_span_tail(&s, nondet_size_t());
            VERIFY_ASSERT(t.p >= s.p && t.p <= t.end && t.end == s.end, "tail: sub-span in bounds");
            break;
        }
        case 6: {
            const char *mark = mkr_span_mark(&s);
            int c = mkr_span_take(&s);
            size_t since = mkr_span_since(&s, mark);
            VERIFY_ASSERT(since == (c >= 0 ? 1u : 0u), "mark/since: counts consumption");
            break;
        }
        }
        VERIFY_ASSERT(s.p >= base && s.p <= s.end && s.end == end,
                      "span invariant: base <= cursor <= end");
    }

    /* Slice helpers over two independent small buffers. */
    {
        char a[4], b[4];
        for (size_t i = 0; i < sizeof a; ++i) { a[i] = (char)nondet_uchar(); b[i] = (char)nondet_uchar(); }
        size_t alen = nondet_size_t(), blen = nondet_size_t();
        VERIFY_ASSUME(alen <= sizeof a && blen <= sizeof b);

        (void)mkr_bytes_eq(a, alen, b, blen);
        size_t idx = 0;
        if (mkr_bytes_find(a, alen, b, blen, &idx))
            VERIFY_ASSERT(idx + blen <= alen, "bytes_find: hit fits in haystack");
        bool empty_hit = mkr_bytes_find(a, alen, NULL, 0, &idx);
        VERIFY_ASSERT(empty_hit && idx == 0, "bytes_find: empty needle matches at 0");
    }
    return 0;
}
