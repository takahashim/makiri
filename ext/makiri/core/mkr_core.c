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

    return 0;
}
