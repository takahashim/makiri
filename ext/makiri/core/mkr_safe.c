#include "mkr_safe.h"

void *
mkr_reallocarray(void *ptr, size_t count, size_t elem)
{
    if (count == 0) {
        free(ptr);
        return NULL;
    }
    size_t bytes;
    if (!mkr_size_mul(count, elem, &bytes)) {
        return NULL; /* overflow: leave ptr unchanged */
    }
    return realloc(ptr, bytes);
}

mkr_status_t
mkr_buf_append(mkr_buf_t *b, const void *bytes, size_t n)
{
    if (n == 0) {
        return MKR_OK;
    }
    size_t need;
    if (!mkr_size_add(b->len, n, &need)) {
        return MKR_ERR_OOM;
    }
    if (b->max != 0 && need > b->max) {
        return MKR_ERR_LIMIT;
    }
    size_t need_term; /* room for the NUL terminator too */
    if (!mkr_size_add(need, 1, &need_term)) {
        return MKR_ERR_OOM;
    }
    if (need_term > b->cap) {
        size_t new_cap;
        if (!mkr_grow_capacity(b->cap, need_term, 1, &new_cap)) {
            return MKR_ERR_OOM;
        }
        char *p = realloc(b->data, new_cap);
        if (p == NULL) {
            return MKR_ERR_OOM;
        }
        b->data = p;
        b->cap  = new_cap;
    }
    memcpy(b->data + b->len, bytes, n);
    b->len += n;
    b->data[b->len] = '\0'; /* keep NUL-terminated */
    return MKR_OK;
}

char *
mkr_buf_steal(mkr_buf_t *b, size_t *out_len)
{
    if (b->data == NULL) {
        char *empty = malloc(1);
        if (empty == NULL) {
            return NULL;
        }
        empty[0] = '\0';
        if (out_len != NULL) *out_len = 0;
        return empty;
    }
    char *p = b->data;
    if (out_len != NULL) *out_len = b->len;
    b->data = NULL;
    b->len  = 0;
    b->cap  = 0;
    return p;
}

mkr_status_t
mkr_vec_push(mkr_vec_t *v, void *item)
{
    if (v->count == v->cap) {
        size_t need;
        if (!mkr_size_add(v->count, 1, &need)) {
            return MKR_ERR_OOM;
        }
        size_t new_cap;
        if (!mkr_grow_capacity(v->cap, need, sizeof(void *), &new_cap)) {
            return MKR_ERR_OOM;
        }
        void **p = mkr_reallocarray(v->items, new_cap, sizeof(void *));
        if (p == NULL) {
            return MKR_ERR_OOM;
        }
        v->items = p;
        v->cap   = new_cap;
    }
    v->items[v->count++] = item;
    return MKR_OK;
}

/* ------------------------------------------------------------------ */
/* self-test                                                          */
/* ------------------------------------------------------------------ */

int
mkr_safe_selftest(void)
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

    /* vec: push + grow */
    {
        mkr_vec_t v;
        mkr_vec_init(&v);
        for (size_t i = 0; i < 100; i++) {
            if (mkr_vec_push(&v, (void *)(uintptr_t)(i + 1)) != MKR_OK) {
                mkr_vec_free(&v);
                return 19;
            }
        }
        if (v.count != 100) { mkr_vec_free(&v); return 20; }
        if (v.items[0] != (void *)(uintptr_t)1 ||
            v.items[99] != (void *)(uintptr_t)100) { mkr_vec_free(&v); return 21; }
        mkr_vec_free(&v);
        if (v.items != NULL) return 22;
    }

    return 0;
}
