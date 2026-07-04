/* CBMC harness: overflow-checked size arithmetic + allocators (core/mkr_alloc).
 *
 * The addition properties hold over ALL inputs. The multiplication properties
 * are proved for an unbounded count times an element size drawn from the
 * concrete set below. The reason is solver-shaped: relating the guard's 64-bit
 * division to the multiplication at full width is intractable for bit-level
 * solvers (measured: >8 min with MiniSat/kissat/Bitwuzla alike), while a
 * few-valued elem turns the division into constant-threshold compares
 * (measured: seconds). The restriction is also faithful to the code: every
 * in-tree caller passes a compile-time sizeof as elem, and the set covers all
 * of them with headroom - extend it if a bigger element type ever appears.
 */
#include "verify.h"
#include "core/mkr_alloc.h"

#include <string.h>

static int
elem_in_set(size_t e)
{
    return e == 1 || e == 2 || e == 4 || e == 8 || e == 16 || e == 24 ||
           e == 32 || e == 48 || e == 64 || e == 96 || e == 128 || e == 256;
}

int
main(void)
{
    /* --- size_add: unbounded --- */
    {
        size_t a = nondet_size_t(), b = nondet_size_t(), out = 0;
        if (mkr_size_add(a, b, &out)) {
            VERIFY_ASSERT(out == a + b, "add: exact");
            VERIFY_ASSERT(out >= a && out >= b, "add: no wrap");
        } else {
            VERIFY_ASSERT(a > SIZE_MAX - b, "add: fails only on true overflow");
        }
    }

    /* --- size_mul: unbounded count x sizeof-set elem (see header comment) --- */
    {
        size_t count = nondet_size_t(), elem = nondet_size_t(), out = 0;
        VERIFY_ASSUME(elem_in_set(elem));
        if (mkr_size_mul(count, elem, &out)) {
            VERIFY_ASSERT(out == count * elem, "mul: exact");
            VERIFY_ASSERT((unsigned __int128)count * elem <= SIZE_MAX, "mul: no wrap");
        } else {
            VERIFY_ASSERT(count != 0 && (unsigned __int128)count * elem > SIZE_MAX,
                          "mul: fails only on true overflow");
        }
    }

    /* --- grow_capacity: unbounded cap/need x sizeof-set elem --- */
    {
        size_t cap = nondet_size_t(), need = nondet_size_t(), elem = nondet_size_t();
        size_t nc = 0;
        VERIFY_ASSUME(elem_in_set(elem));
        if (mkr_grow_capacity(cap, need, elem, &nc)) {
            VERIFY_ASSERT(nc >= need, "grow_capacity: covers need");
            VERIFY_ASSERT((unsigned __int128)nc * elem <= SIZE_MAX, "grow_capacity: nc*elem fits");
        }
    }

    /* --- heap paths: bounded sizes, malloc may fail --- */
    {
        size_t n = nondet_size_t();
        VERIFY_ASSUME(n <= 8);
        char *s = mkr_str_alloc(n);
        if (s != NULL) {
            VERIFY_ASSERT(s[n] == '\0', "str_alloc: terminator pre-set");
            for (size_t i = 0; i < n; ++i) s[i] = 'x'; /* content writable */
            free(s);
        }
    }
    {
        unsigned char src[4];
        for (size_t i = 0; i < sizeof src; ++i) src[i] = nondet_uchar();
        size_t n = nondet_size_t();
        VERIFY_ASSUME(n <= sizeof src);
        char *s = mkr_strndup((const char *)src, n);
        if (s != NULL) {
            VERIFY_ASSERT(s[n] == '\0', "strndup: terminated");
            VERIFY_ASSERT(n == 0 || memcmp(s, src, n) == 0, "strndup: copies");
            free(s);
        }
        VERIFY_ASSERT(mkr_strndup(NULL, 1) == NULL, "strndup: NULL src with n>0 fails closed");
    }
    {
        /* strdup: copies up to the first NUL, terminated; NULL -> NULL. The
         * source is force-terminated inside the buffer so strlen is bounded.
         * (unsigned char storage + a pointer cast: a VALUE cast of a >127 byte
         * to char is implementation-defined and --conversion-check flags it.) */
        unsigned char src[4];
        for (size_t i = 0; i + 1 < sizeof src; ++i) src[i] = nondet_uchar();
        src[sizeof src - 1] = '\0';
        char *d = mkr_strdup((const char *)src);
        if (d != NULL) {
            size_t n = strlen((const char *)src);
            VERIFY_ASSERT(d[n] == '\0', "strdup: terminated");
            VERIFY_ASSERT(n == 0 || memcmp(d, src, n) == 0, "strdup: copies");
            free(d);
        }
        VERIFY_ASSERT(mkr_strdup(NULL) == NULL, "strdup: NULL -> NULL");
    }
    {
        /* str_alloc at ANY n: the n + 1 sizing is right (the terminator slot
         * exists) or the call fails closed with NULL - the off-by-one class,
         * proved at full range (the bounded block above covers the content
         * writes). */
        size_t n = nondet_size_t();
        char *s = mkr_str_alloc(n);
        if (s != NULL) {
            VERIFY_ASSERT(s[n] == '\0', "str_alloc: terminator slot exists at any n");
            if (n > 0) s[0] = 'x';
            free(s);
        }
    }
    {
        /* reallocarray's MOVE path: growing a live allocation preserves the
         * prefix and makes the new tail writable; a failed grow leaves the
         * original owned and intact; count == 0 frees and returns NULL. */
        uint32_t *p = mkr_reallocarray(NULL, 2, sizeof(uint32_t));
        if (p != NULL) {
            p[0] = 0x11111111u; p[1] = 0x22222222u;
            uint32_t *q = mkr_reallocarray(p, 4, sizeof(uint32_t));
            if (q != NULL) {
                VERIFY_ASSERT(q[0] == 0x11111111u && q[1] == 0x22222222u,
                              "reallocarray: grow preserves content");
                q[3] = 0x33333333u; /* new tail writable */
                VERIFY_ASSERT(mkr_reallocarray(q, 0, sizeof(uint32_t)) == NULL,
                              "reallocarray: count 0 frees and returns NULL");
            } else {
                VERIFY_ASSERT(p[0] == 0x11111111u && p[1] == 0x22222222u,
                              "reallocarray: failed grow leaves the original intact");
                free(p);
            }
        }
    }
    {
        void  *arr = NULL;
        size_t cap = 0;
        size_t need = nondet_size_t();
        VERIFY_ASSUME(need <= 4);
        if (mkr_grow_reserve(&arr, &cap, need, sizeof(uint32_t)) == MKR_OK) {
            VERIFY_ASSERT(cap >= need, "grow_reserve: capacity covers need");
            uint32_t *w = (uint32_t *)arr;
            for (size_t i = 0; i < need; ++i) w[i] = 0; /* in-bounds writes */
        } else {
            VERIFY_ASSERT(arr == NULL && cap == 0, "grow_reserve: fail leaves state unchanged");
        }
        free(arr);
    }
    {
        size_t count = nondet_size_t(), elem = nondet_size_t();
        VERIFY_ASSUME(count <= 4 && elem <= 4);
        void *p = mkr_callocarray(count, elem);
        if (p != NULL) {
            const unsigned char *z = (const unsigned char *)p;
            for (size_t i = 0; i < count * elem; ++i)
                VERIFY_ASSERT(z[i] == 0, "callocarray: zeroed");
            free(p);
        }
    }

    /* --- fail-closed on a true overflow: allocators must return NULL.
     * (Last: the overflow assumption is unreachable under the smoke build's
     * small pseudo-inputs, and an unmet assume ends the smoke run.) --- */
    {
        size_t count = nondet_size_t(), elem = nondet_size_t();
        VERIFY_ASSUME(elem_in_set(elem));
        VERIFY_ASSUME((unsigned __int128)count * elem > SIZE_MAX);
        VERIFY_ASSERT(mkr_callocarray(count, elem) == NULL, "callocarray: overflow fails closed");
        VERIFY_ASSERT(mkr_reallocarray(NULL, count, elem) == NULL, "reallocarray: overflow fails closed");
    }
    return 0;
}
