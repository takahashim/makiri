#include "mkr_core.h"

/* ------------------------------------------------------------------ */
/* self-test                                                          */
/* ------------------------------------------------------------------ */

/* Exercises the whole core (mkr_alloc + mkr_buf), so it lives at the umbrella
 * level rather than in either mkr_alloc.c or mkr_buf.c. Wired to a private Ruby
 * method (Makiri.__c_selftest) for the spec suite. */
int
mkr_core_selftest(void)
{
    size_t out = 0;

    /* size_add / size_mul: normal and overflow */
    if (!mkr_size_add(2, 3, &out) || out != 5) return 1;
    if (mkr_size_add(SIZE_MAX, 1, &out)) return 2;          /* must overflow */
    if (!mkr_size_mul(4, 5, &out) || out != 20) return 3;
    if (mkr_size_mul(SIZE_MAX / 2 + 1, 2, &out)) return 4;  /* must overflow */
    if (!mkr_size_mul(0, SIZE_MAX, &out) || out != 0) return 5;
    /* exact-boundary: the largest non-overflowing result MUST succeed (catches an
     * off-by-one in the `a > SIZE_MAX - b` / `b > SIZE_MAX / a` predicates). */
    if (!mkr_size_add(SIZE_MAX, 0, &out) || out != SIZE_MAX) return 55;
    if (!mkr_size_add(SIZE_MAX - 1, 1, &out) || out != SIZE_MAX) return 56;
    if (!mkr_size_mul(SIZE_MAX, 1, &out) || out != SIZE_MAX) return 57;
    if (!mkr_size_mul(SIZE_MAX / 2, 2, &out) || out != (SIZE_MAX / 2) * 2) return 58;

    /* grow_capacity: geometric, and overflow on a huge element */
    if (!mkr_grow_capacity(0, 1, 1, &out) || out < 1) return 6;
    if (!mkr_grow_capacity(8, 9, 1, &out) || out < 9) return 7;
    if (mkr_grow_capacity(0, SIZE_MAX, 2, &out)) return 8;  /* nc*elem overflows */

    /* reallocarray overflow returns NULL without touching ptr */
    if (mkr_reallocarray(NULL, SIZE_MAX, 2) != NULL) return 9;

    /* callocarray: zeroed, and NULL on overflow / zero count */
    {
        size_t *a = mkr_callocarray(4, sizeof(*a));
        if (a == NULL || a[0] != 0 || a[3] != 0) { free(a); return 23; }
        free(a);
        if (mkr_callocarray(SIZE_MAX, 2) != NULL) return 24; /* overflow */
        if (mkr_callocarray(0, 4) != NULL) return 25;         /* zero count */
    }

    /* str_alloc / strndup / strdup: copy, terminate, fail closed on NULL+len */
    {
        if (mkr_str_alloc(SIZE_MAX) != NULL) return 26;          /* n + 1 overflow */
        char *z = mkr_str_alloc(0);                              /* boundary: 0 -> 1-byte "\0" */
        if (z == NULL || z[0] != '\0') { free(z); return 59; }
        free(z);
        char *p = mkr_strndup("hello", 3);
        if (p == NULL || memcmp(p, "hel", 4) != 0) { free(p); return 27; } /* "hel\0" */
        free(p);
        if (mkr_strndup(NULL, 4) != NULL) return 28;             /* NULL + nonzero -> NULL */
        char *e = mkr_strndup(NULL, 0);                          /* NULL + zero -> "" */
        if (e == NULL || e[0] != '\0') { free(e); return 29; }
        free(e);
        if (mkr_strdup(NULL) != NULL) return 30;
        char *d = mkr_strdup("xy");
        if (d == NULL || memcmp(d, "xy", 3) != 0) { free(d); return 31; }
        free(d);
    }

    /* buf: NULL source with nonzero length -> INVALID; n == 0 is a no-op */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 0);
        if (mkr_buf_append(&b, NULL, 3) != MKR_ERR_INVALID) { mkr_buf_free(&b); return 32; }
        if (mkr_buf_append(&b, NULL, 0) != MKR_OK) { mkr_buf_free(&b); return 33; }
        if (b.len != 0) { mkr_buf_free(&b); return 34; }
        mkr_buf_free(&b);
    }

    /* buf: append, terminate, steal */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 0);
        if (mkr_buf_append(&b, "ab", 2) != MKR_OK) return 10;
        if (mkr_buf_append(&b, "c", 1) != MKR_OK) return 11;
        if (b.len != 3 || b.data == NULL || b.data[3] != '\0') return 12;
        if (memcmp(b.data, "abc", 3) != 0) return 13;
        size_t n = 0;
        char *s = mkr_buf_steal(&b, &n);
        if (s == NULL || n != 3 || memcmp(s, "abc", 4) != 0) { free(s); return 14; }
        free(s);
        if (b.data != NULL || b.len != 0) return 15;
        mkr_buf_free(&b);
    }

    /* buf: cap (max) -> LIMIT */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 4);
        if (mkr_buf_append(&b, "abcd", 4) != MKR_OK) return 16; /* exactly max ok */
        if (mkr_buf_append(&b, "e", 1) != MKR_ERR_LIMIT) return 17;
        mkr_buf_free(&b);
    }

    /* buf: empty steal yields an owned "" */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 0);
        size_t n = 1;
        char *s = mkr_buf_steal(&b, &n);
        if (s == NULL || n != 0 || s[0] != '\0') { free(s); return 18; }
        free(s);
    }

    /* spanbuf: writes that fit, then one that overruns -> refused + sticky */
    {
        char store[4];
        mkr_spanbuf_t f = mkr_spanbuf(store, sizeof store);
        mkr_spanbuf_putc(&f, 'a');
        mkr_spanbuf_write(&f, "bc", 2);                  /* pos == 3, room for 1 */
        if (!f.ok || f.pos != 3) return 35;
        mkr_spanbuf_write(&f, "de", 2);                  /* exceeds cap -> refused */
        if (f.ok) return 36;                              /* must have latched */
        if (f.pos != 3) return 37;                        /* refused write didn't advance */
        if (mkr_spanbuf_finish(&f) != NULL) return 38;   /* not-ok -> NULL */
        if (memcmp(store, "abc", 3) != 0) return 39;      /* no overrun past pos */
    }

    /* spanbuf: exact fill is ok; a NULL backing buffer is never ok */
    {
        char store[2];
        mkr_spanbuf_t f = mkr_spanbuf(store, sizeof store);
        mkr_spanbuf_write(&f, "xy", 2);                  /* exactly cap -> still ok */
        if (!f.ok || mkr_spanbuf_finish(&f) != store || f.pos != 2) return 40;
        mkr_spanbuf_putc(&f, 'z');                       /* boundary: at pos==cap -> refused */
        if (f.ok || f.pos != 2) return 60;

        mkr_spanbuf_t g = mkr_spanbuf(NULL, 8);         /* alloc-failed backing */
        mkr_spanbuf_putc(&g, 'z');                       /* no-op, no crash */
        if (g.ok || mkr_spanbuf_finish(&g) != NULL) return 41;
    }

    /* grow_reserve: grows + updates ptr/cap; already-enough is a no-op; an
     * overflowing (need*elem) request fails closed (OOM) leaving ptr/cap as-is. */
    {
        size_t *p = NULL, cap = 0;
        if (mkr_grow_reserve((void **)&p, &cap, 10, sizeof(*p)) != MKR_OK) { free(p); return 42; }
        if (p == NULL || cap < 10) { free(p); return 43; }
        p[0] = 1; p[9] = 2;                                 /* in-bounds after grow */

        size_t *d0 = p; size_t c0 = cap;
        if (mkr_grow_reserve((void **)&p, &cap, 5, sizeof(*p)) != MKR_OK) { free(p); return 44; }
        if (p != d0 || cap != c0) { free(p); return 45; }   /* need < cap -> no-op */
        if (mkr_grow_reserve((void **)&p, &cap, cap, sizeof(*p)) != MKR_OK) { free(p); return 61; }
        if (p != d0 || cap != c0) { free(p); return 62; }   /* boundary: need == cap -> no-op */

        if (mkr_grow_reserve((void **)&p, &cap, SIZE_MAX, sizeof(*p)) != MKR_ERR_OOM) { free(p); return 46; }
        if (p != d0 || cap != c0) { free(p); return 47; }   /* overflow -> unchanged */
        free(p);
    }

    /* buf_reserve: pre-allocates capacity without touching len; no-op when room
     * already exists; clamps a request to the buffer's max (never allocates past
     * it); the buffer stays usable afterwards. */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 16);                                /* small ceiling */
        if (mkr_buf_reserve(&b, 8) != MKR_OK) { mkr_buf_free(&b); return 48; }
        if (b.cap < 9 || b.len != 0) { mkr_buf_free(&b); return 49; } /* reserved (+NUL), still empty */

        char *d0 = b.data; size_t c0 = b.cap;
        if (mkr_buf_reserve(&b, 4) != MKR_OK) { mkr_buf_free(&b); return 50; }
        if (b.data != d0 || b.cap != c0) { mkr_buf_free(&b); return 51; } /* already room -> no-op */

        if (mkr_buf_reserve(&b, (size_t)1 << 40) != MKR_OK) { mkr_buf_free(&b); return 52; }
        if (b.cap > 17) { mkr_buf_free(&b); return 53; }     /* clamped to max(16)+NUL, no huge alloc */

        if (mkr_buf_append(&b, "abc", 3) != MKR_OK || b.len != 3) { mkr_buf_free(&b); return 54; }
        mkr_buf_free(&b);
    }

    /* buf_append: geometric growth is clamped to the content ceiling (max + the
     * NUL), so a near-limit append never over-allocates to ~2x. Here filling to
     * max(10) would geometrically reach cap 16; the clamp holds it at max+1(11),
     * the same property that keeps a real buffer's cap under MKR_BUF_HARD_MAX. */
    {
        mkr_buf_t b;
        mkr_buf_init(&b, 10);
        if (mkr_buf_append(&b, "0123456789", 10) != MKR_OK) { mkr_buf_free(&b); return 63; }
        if (b.len != 10 || b.cap > 11) { mkr_buf_free(&b); return 64; }  /* clamped, not the geometric 16 */
        if (mkr_buf_append(&b, "x", 1) != MKR_ERR_LIMIT) { mkr_buf_free(&b); return 65; } /* still capped */
        mkr_buf_free(&b);
    }

    return 0;
}
