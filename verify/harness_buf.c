/* CBMC harness: the growable capped buffer (core/mkr_buf.c) checked against a
 * shadow reference model.
 *
 * A small nondet max (1..6) makes the LIMIT ceiling reachable; two nondet
 * appends with a reserve in between drive the growth, clamp and failure paths
 * (run with --malloc-may-fail --malloc-fail-null so every OOM branch is
 * covered). Properties:
 *   - a successful append extends the shadow copy exactly (content equality),
 *     keeps the NUL terminator, and never takes len past the ceiling;
 *   - cap never exceeds max + 1 (the geometric-growth clamp: no ~2x overshoot
 *     near the limit - the property that bounds a real buffer's allocation by
 *     MKR_BUF_HARD_MAX);
 *   - every failure (INVALID / LIMIT / OOM) leaves len, cap and the content
 *     bytes untouched;
 *   - steal hands back the exact accumulated content, NUL-terminated, and
 *     resets the buffer to a usable empty state.
 */
#include "verify.h"
#include "core/mkr_buf.h"

#include <string.h>

#define MAXCAP 6
#define NSRC   4

/* One nondet append checked against the shadow model. */
static void
step_append(mkr_buf_t *b, char *shadow, size_t *slen, size_t maxlim)
{
    unsigned char src[NSRC];
    for (size_t i = 0; i < sizeof src; ++i) src[i] = nondet_uchar();
    size_t n = nondet_size_t();
    if (n > NSRC) n = NSRC;

    size_t len0 = b->len, cap0 = b->cap;
    mkr_status_t st = mkr_buf_append(b, src, n);
    if (st == MKR_OK) {
        VERIFY_ASSERT(b->len == len0 + n, "append: len advances by n");
        VERIFY_ASSERT(b->len <= maxlim, "append: ceiling respected");
        VERIFY_ASSERT(b->cap <= maxlim + 1, "append: growth clamped to max+1");
        VERIFY_ASSERT(n == 0 || b->data[b->len] == '\0', "append: NUL-terminated");
        memcpy(shadow + *slen, src, n);
        *slen += n;
        VERIFY_ASSERT(*slen == 0 || memcmp(b->data, shadow, *slen) == 0,
                      "append: content matches the shadow model");
    } else {
        VERIFY_ASSERT(st == MKR_ERR_LIMIT || st == MKR_ERR_OOM,
                      "append: non-NULL src fails only on limit/OOM");
        VERIFY_ASSERT(st != MKR_ERR_LIMIT || len0 + n > maxlim,
                      "append: LIMIT only past the ceiling");
        VERIFY_ASSERT(b->len == len0 && b->cap == cap0, "append: failure leaves size intact");
        VERIFY_ASSERT(*slen == 0 || memcmp(b->data, shadow, *slen) == 0,
                      "append: failure leaves content intact");
    }
}

int
main(void)
{
    size_t max = nondet_size_t();
    VERIFY_ASSUME(max >= 1 && max <= MAXCAP); /* small ceiling so LIMIT is reachable */

    mkr_buf_t b;
    mkr_buf_init(&b, max);

    char   shadow[2 * NSRC];
    size_t slen = 0;

    /* NULL source with n > 0 is INVALID and touches nothing. */
    VERIFY_ASSERT(mkr_buf_append(&b, NULL, 3) == MKR_ERR_INVALID, "append: NULL src fails closed");
    VERIFY_ASSERT(b.len == 0 && b.cap == 0, "append: INVALID touches nothing");

    step_append(&b, shadow, &slen, max);

    /* reserve between appends: len/content unchanged, cap stays clamped. */
    {
        size_t len0 = b.len;
        (void)mkr_buf_reserve(&b, nondet_size_t() % (2 * MAXCAP));
        VERIFY_ASSERT(b.len == len0, "reserve: len untouched");
        VERIFY_ASSERT(b.cap <= max + 1, "reserve: clamped to the buffer's ceiling");
        VERIFY_ASSERT(slen == 0 || memcmp(b.data, shadow, slen) == 0,
                      "reserve: content untouched");
    }

    step_append(&b, shadow, &slen, max);

    /* reserve return-value contract: success leaves len/content untouched and
     * never shrinks cap (still clamped to the buffer's own ceiling); failure
     * (OOM only - reserve never fails INVALID/LIMIT) leaves len, cap AND
     * content untouched. */
    {
        size_t len0 = b.len, cap0 = b.cap;
        char   before[2 * NSRC];
        if (b.len > 0) memcpy(before, b.data, b.len);
        mkr_status_t st = mkr_buf_reserve(&b, nondet_size_t() % (2 * MAXCAP));
        if (st == MKR_OK) {
            VERIFY_ASSERT(b.len == len0, "reserve: success leaves len unchanged");
            VERIFY_ASSERT(b.cap >= cap0, "reserve: success does not shrink cap");
            VERIFY_ASSERT(b.cap <= max + 1, "reserve: cap stays clamped");
        } else {
            VERIFY_ASSERT(st == MKR_ERR_OOM, "reserve: failure is OOM");
            VERIFY_ASSERT(b.len == len0 && b.cap == cap0, "reserve: failure leaves len/cap");
            VERIFY_ASSERT(b.len == 0 || memcmp(b.data, before, b.len) == 0,
                          "reserve: failure leaves content");
        }
    }

    /* Ceiling SELECTION at ANY max: max == 0 falls back to the default limit
     * (not unbounded), max past MKR_BUF_HARD_MAX is clamped to it, and a huge
     * reserve request never allocates past limit + 1 (the "no huge alloc"
     * guarantee, at full range - the model-checked block above only sees
     * max 1..6). Small appends keep the loops bounded; the ceiling arithmetic
     * is what runs at full width. */
    {
        mkr_buf_t h;
        mkr_buf_init(&h, nondet_size_t()); /* ANY max, 0 included */
        size_t soft  = (h.max != 0) ? h.max : MKR_BUF_DEFAULT_LIMIT;
        size_t limit = (soft < MKR_BUF_HARD_MAX) ? soft : MKR_BUF_HARD_MAX;

        if (mkr_buf_reserve(&h, nondet_size_t()) == MKR_OK)  /* ANY request */
            VERIFY_ASSERT(h.cap <= limit + 1, "reserve: any request clamps to limit+1");
        VERIFY_ASSERT(h.len == 0, "reserve: len untouched at any max");

        mkr_status_t st = mkr_buf_append(&h, "ab", 2);
        if (st == MKR_OK) {
            VERIFY_ASSERT(h.len == 2 && h.data[2] == '\0', "append: works under any ceiling");
            VERIFY_ASSERT(h.cap <= limit + 1, "append: growth clamped at any max");
        } else {
            VERIFY_ASSERT(st == MKR_ERR_LIMIT ? limit < 2 : st == MKR_ERR_OOM,
                          "append: fails closed only on limit/OOM");
        }
        mkr_buf_free(&h);
    }

    /* steal: exact content ownership transfer + reset to a usable buffer. */
    {
        size_t out_len = (size_t)-1;
        char *s = mkr_buf_steal(&b, &out_len);
        if (s != NULL) {
            VERIFY_ASSERT(out_len == slen, "steal: reports the accumulated length");
            VERIFY_ASSERT(s[out_len] == '\0', "steal: NUL-terminated");
            VERIFY_ASSERT(slen == 0 || memcmp(s, shadow, slen) == 0, "steal: exact content");
            free(s);
        }
        VERIFY_ASSERT(b.data == NULL && b.len == 0 && b.cap == 0, "steal: buffer reset");
        VERIFY_ASSERT(mkr_buf_append(&b, "z", 1) != MKR_ERR_INVALID, "steal: buffer stays usable");
    }
    mkr_buf_free(&b);

    /* steal on an empty (never-appended-to) buffer: success hands back an
     * owned "", OOM (on the internal malloc(1)) leaves out_len untouched (the
     * impl only sets *out_len on the success path) and the buffer empty. */
    {
        mkr_buf_t e;
        mkr_buf_init(&e, 0);
        size_t out_len = (size_t)0xABCD; /* sentinel */
        char *s = mkr_buf_steal(&e, &out_len);
        if (s != NULL) {
            VERIFY_ASSERT(out_len == 0 && s[0] == '\0', "steal empty: owned \"\", len 0");
            free(s);
        } else {
            VERIFY_ASSERT(out_len == (size_t)0xABCD, "steal empty OOM: out_len untouched");
            VERIFY_ASSERT(e.data == NULL && e.len == 0, "steal empty OOM: buffer stays empty");
        }
    }
    return 0;
}
