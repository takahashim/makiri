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
    return 0;
}
