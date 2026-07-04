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

/* Reference model for mkr_bytes_find: first-match linear scan, independent of
 * the engine's own inner loop. */
static bool
ref_find(const char *hay, size_t hay_len, const char *needle, size_t needle_len, size_t *idx)
{
    if (needle_len == 0) { *idx = 0; return true; }
    if (needle_len > hay_len) return false;
    for (size_t i = 0; i + needle_len <= hay_len; ++i) {
        bool match = true;
        for (size_t j = 0; j < needle_len; ++j) {
            if (hay[i + j] != needle[j]) { match = false; break; }
        }
        if (match) { *idx = i; return true; }
    }
    return false;
}

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

    /* #7: mkr_span_starts against a reference model - n == 0 is true even for
     * a NULL literal, a too-short remainder is false, otherwise it's exactly
     * a memcmp of the first n bytes. */
    {
        char sbuf[6];
        for (size_t i = 0; i < sizeof sbuf; ++i) sbuf[i] = (char)nondet_uchar();
        size_t slen = nondet_size_t();
        VERIFY_ASSUME(slen <= sizeof sbuf);
        mkr_span_t ss = mkr_span(sbuf, slen);

        size_t adv = nondet_size_t();
        VERIFY_ASSUME(adv <= slen);
        mkr_span_skip(&ss, adv);

        char lit[4];
        for (size_t i = 0; i < sizeof lit; ++i) lit[i] = (char)nondet_uchar();
        size_t n = nondet_size_t();
        VERIFY_ASSUME(n <= sizeof lit);

        size_t left = mkr_span_left(&ss);
        bool got = mkr_span_starts(&ss, lit, n);
        bool expect = (n == 0) || (left >= n && memcmp(ss.p, lit, n) == 0);
        VERIFY_ASSERT(got == expect, "span_starts: matches reference model");

        VERIFY_ASSERT(mkr_span_starts(&ss, NULL, 0), "span_starts: n==0 true even with NULL lit");
    }

    /* #8: mkr_bytes_eq against a reference model, plus the zero-length-NULL
     * and unequal-length boundary cases. */
    {
        char a2[4], b2[4];
        for (size_t i = 0; i < sizeof a2; ++i) { a2[i] = (char)nondet_uchar(); b2[i] = (char)nondet_uchar(); }
        size_t alen2 = nondet_size_t(), blen2 = nondet_size_t();
        VERIFY_ASSUME(alen2 <= sizeof a2 && blen2 <= sizeof b2);

        bool expect = (alen2 == blen2);
        if (expect) {
            for (size_t i = 0; i < alen2; ++i)
                if (a2[i] != b2[i]) expect = false;
        }
        VERIFY_ASSERT(mkr_bytes_eq(a2, alen2, b2, blen2) == expect, "bytes_eq: matches reference model");

        VERIFY_ASSERT(mkr_bytes_eq(NULL, 0, NULL, 0), "bytes_eq: zero-length NULLs are equal");

        size_t xlen = nondet_size_t(), ylen = nondet_size_t();
        VERIFY_ASSUME(xlen <= sizeof a2 && ylen <= sizeof b2 && xlen != ylen);
        VERIFY_ASSERT(!mkr_bytes_eq(a2, xlen, b2, ylen), "bytes_eq: different lengths are never equal");
    }

    /* #9: mkr_bytes_find against ref_find - same hit/miss verdict, first-match
     * offset on a hit, sentinel left untouched on a miss. */
    {
        unsigned char hay[5], needle[3];
        for (size_t i = 0; i < sizeof hay; ++i) hay[i] = nondet_uchar();
        for (size_t i = 0; i < sizeof needle; ++i) needle[i] = nondet_uchar();
        size_t hl = nondet_size_t(), nl = nondet_size_t();
        VERIFY_ASSUME(hl <= sizeof hay && nl <= sizeof needle);

        size_t sentinel = (size_t)0xABCD;
        size_t idx = sentinel;
        bool got = mkr_bytes_find(hay, hl, needle, nl, &idx);

        size_t exp_idx = 0;
        bool exp = ref_find((const char *)hay, hl, (const char *)needle, nl, &exp_idx);

        VERIFY_ASSERT(got == exp, "bytes_find: matches reference model");
        if (got) {
            VERIFY_ASSERT(idx == exp_idx, "bytes_find: returns first match");
        } else {
            VERIFY_ASSERT(idx == sentinel, "bytes_find: miss leaves idx unchanged");
        }
    }

    /* #10: mkr_span(NULL, nonzero) normalizes to a valid empty span - every
     * read is -1/false/no-op regardless of the requested length. */
    {
        mkr_span_t s10 = mkr_span(NULL, nondet_size_t());
        VERIFY_ASSERT(mkr_span_left(&s10) == 0, "span NULL: left zero");
        VERIFY_ASSERT(mkr_span_peek(&s10) == -1, "span NULL: peek -1");
        VERIFY_ASSERT(mkr_span_take(&s10) == -1, "span NULL: take -1");
        mkr_span_skip(&s10, nondet_size_t());
        VERIFY_ASSERT(mkr_span_left(&s10) == 0, "span NULL: skip stays empty");
    }

    /* #11: mkr_span_since after a short nondet op sequence equals the raw
     * pointer difference and never exceeds the length available at the mark. */
    {
        char buf11[6];
        for (size_t i = 0; i < sizeof buf11; ++i) buf11[i] = (char)nondet_uchar();
        size_t len11 = nondet_size_t();
        VERIFY_ASSUME(len11 <= sizeof buf11);
        mkr_span_t s11 = mkr_span(buf11, len11);
        size_t orig_left = mkr_span_left(&s11);
        const char *mark11 = mkr_span_mark(&s11);

        for (int i = 0; i < 3; ++i) {
            unsigned char op = nondet_uchar();
            if (op % 2 == 0) {
                mkr_span_skip(&s11, nondet_size_t());
            } else {
                (void)mkr_span_take(&s11);
            }
        }

        size_t since = mkr_span_since(&s11, mark11);
        VERIFY_ASSERT(since == (size_t)(s11.p - mark11), "since: equals pointer difference");
        VERIFY_ASSERT(since <= orig_left, "since: bounded by original remaining length");
    }

    return 0;
}
