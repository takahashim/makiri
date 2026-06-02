#ifndef MAKIRI_CORE_MKR_SAFE_H
#define MAKIRI_CORE_MKR_SAFE_H

/*
 * Ruby-free safety primitives shared by every C layer (glue, xpath engine,
 * lexbor_compat). Overflow-checked size arithmetic, a capped growable byte
 * buffer (mkr_buf_t), and a growable pointer vector (mkr_vec_t) so that the
 * ad-hoc `cap *= 2` / `n + 1` / `malloc(n * sizeof(T))` patterns are written
 * once, here, and fail closed. NOTHING in this header touches Ruby — exception
 * mapping happens at the glue boundary.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Status for the Ruby-free layers. The glue boundary maps these to Ruby
 * exceptions; the xpath engine maps to mkr_xpath_error_t. */
typedef enum {
    MKR_OK = 0,
    MKR_ERR_OOM,
    MKR_ERR_LIMIT,
    MKR_ERR_INVALID,
    MKR_ERR_INTERNAL
} mkr_status_t;

/* Overflow-checked size arithmetic. Return false on overflow (then *out is
 * left unchanged). */
static inline bool
mkr_size_add(size_t a, size_t b, size_t *out)
{
    if (a > SIZE_MAX - b) return false;
    *out = a + b;
    return true;
}

static inline bool
mkr_size_mul(size_t a, size_t b, size_t *out)
{
    if (a != 0 && b > SIZE_MAX / a) return false;
    *out = a * b;
    return true;
}

/* Compute a new capacity (in elements) that is >= need and grows geometrically
 * from cap, with cap*elem guaranteed to fit in size_t. Returns false on
 * overflow. */
static inline bool
mkr_grow_capacity(size_t cap, size_t need, size_t elem, size_t *new_cap)
{
    size_t nc = (cap != 0) ? cap : 8;
    while (nc < need) {
        if (nc > SIZE_MAX / 2) { nc = need; break; } /* avoid *2 overflow */
        nc *= 2;
    }
    size_t bytes;
    if (!mkr_size_mul(nc, elem, &bytes)) return false;
    *new_cap = nc;
    return true;
}

/* realloc(ptr, count * elem) with the multiply overflow-checked. Returns NULL
 * on overflow or OOM; the original ptr is then unchanged (caller still owns
 * it). count == 0 frees and returns NULL. */
void *mkr_reallocarray(void *ptr, size_t count, size_t elem);

/* calloc(count, elem) with the multiply overflow-checked, result zeroed.
 * Returns NULL on overflow, allocation failure, or count == 0 / elem == 0. */
void *mkr_callocarray(size_t count, size_t elem);

/* Ensure the array at *ptr (currently *cap elements of `elem` bytes) can hold
 * at least `need` elements, growing geometrically and overflow-safely. On
 * success updates *ptr / *cap and returns MKR_OK; on overflow/allocation failure
 * returns MKR_ERR_OOM with *ptr / *cap unchanged. Replaces the hand-rolled
 * `cap *= 2; realloc(p, cap * sizeof(T))` pattern in one call. */
mkr_status_t mkr_grow_reserve(void **ptr, size_t *cap, size_t need, size_t elem);

/* ---------------------------------------------------------------- */
/* mkr_buf_t — owned, growable, optionally capped byte buffer        */
/* ---------------------------------------------------------------- */

typedef struct {
    char  *data; /* owned; kept NUL-terminated after any append */
    size_t len;  /* bytes used (excluding the terminator) */
    size_t cap;  /* bytes allocated */
    size_t max;  /* 0 = unbounded; else append past max returns MKR_ERR_LIMIT */
} mkr_buf_t;

/* Initialise an empty buffer. max == 0 means unbounded. */
static inline void
mkr_buf_init(mkr_buf_t *b, size_t max)
{
    b->data = NULL;
    b->len  = 0;
    b->cap  = 0;
    b->max  = max;
}

/* Append n bytes. Fails closed: MKR_ERR_LIMIT if it would exceed max,
 * MKR_ERR_OOM on overflow or allocation failure (the buffer is left intact). */
mkr_status_t mkr_buf_append(mkr_buf_t *b, const void *bytes, size_t n);

/* Take ownership of the (NUL-terminated) bytes; the buffer is reset to empty.
 * Returns a freshly owned "" for an empty buffer, or NULL on OOM. */
char *mkr_buf_steal(mkr_buf_t *b, size_t *out_len);

/* Free the buffer's storage. Safe on a zero-initialised / stolen buffer. */
static inline void
mkr_buf_free(mkr_buf_t *b)
{
    free(b->data);
    b->data = NULL;
    b->len = b->cap = 0;
}

/* ---------------------------------------------------------------- */
/* mkr_vec_t — owned, growable pointer vector                        */
/* ---------------------------------------------------------------- */

typedef struct {
    void **items;
    size_t count;
    size_t cap;
} mkr_vec_t;

static inline void
mkr_vec_init(mkr_vec_t *v)
{
    v->items = NULL;
    v->count = 0;
    v->cap   = 0;
}

/* Append item. MKR_ERR_OOM on overflow/allocation failure (vec left intact). */
mkr_status_t mkr_vec_push(mkr_vec_t *v, void *item);

static inline void
mkr_vec_free(mkr_vec_t *v)
{
    free(v->items);
    v->items = NULL;
    v->count = v->cap = 0;
}

/* ---------------------------------------------------------------- */
/* ownership-bearing views                                          */
/* ---------------------------------------------------------------- */

/* Borrowed (non-owning) slice of bytes. */
typedef struct { const char *ptr; size_t len; } mkr_str_view_t;

/* Owned bytes. */
typedef struct { char *ptr; size_t len; } mkr_owned_bytes_t;

static inline void
mkr_owned_bytes_clear(mkr_owned_bytes_t *o)
{
    free(o->ptr);
    o->ptr = NULL;
    o->len = 0;
}

/* Self-test of the overflow / buffer / vector edge cases (incl. paths real
 * inputs cannot reach). Returns 0 on success, nonzero on the first failure.
 * Wired to a private Ruby method for the spec suite. */
int mkr_safe_selftest(void);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_SAFE_H */
