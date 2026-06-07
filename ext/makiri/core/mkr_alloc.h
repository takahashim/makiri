#ifndef MAKIRI_CORE_MKR_ALLOC_H
#define MAKIRI_CORE_MKR_ALLOC_H

/*
 * Fail-closed memory primitives: overflow-checked size arithmetic and
 * allocators, the foundation every other C layer (glue, xpath engine,
 * lexbor_compat) builds on, so the ad-hoc `cap *= 2` / `n + 1` /
 * `malloc(n * sizeof(T))` patterns are written once, here, and fail closed.
 * NOTHING in this header touches Ruby - exception mapping happens at the glue
 * boundary. (mkr_core.h is a thin umbrella over this + the other core headers.)
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

/* Overflow-checked size arithmetic. Return false on overflow (then *out is* left unchanged). */
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

/* Allocate a NUL-terminable string buffer with room for n content bytes plus
 * the terminator (n + 1 bytes), the size overflow-checked. The n content bytes
 * are uninitialised (the terminator slot at [n] is pre-set to '\0'); the caller
 * fills the content. Returns NULL on overflow / OOM. */
char *mkr_str_alloc(size_t n);

/* Copy n bytes from s into a fresh NUL-terminated buffer. s must be non-NULL and
 * hold n bytes when n > 0 (s == NULL with n > 0 fails closed -> NULL); s may be
 * NULL when n == 0 (yields an owned ""). Returns NULL on overflow / OOM. */
char *mkr_strndup(const char *s, size_t n);

/* strdup replacement: s == NULL -> NULL; otherwise an owned NUL-terminated copy,
 * or NULL on overflow / OOM. Note: relies on strlen, so s MUST be a genuine
 * NUL-terminated C string. When the length is already known (a borrowed slice,
 * a verified_text), use mkr_strndup(ptr, len) instead. */
char *mkr_strdup(const char *s);

/* Ensure the array at *ptr (currently *cap elements of `elem` bytes) can hold
 * at least `need` elements, growing geometrically and overflow-safely. On
 * success updates *ptr / *cap and returns MKR_OK; on overflow/allocation failure
 * returns MKR_ERR_OOM with *ptr / *cap unchanged. Replaces the hand-rolled
 * `cap *= 2; realloc(p, cap * sizeof(T))` pattern in one call. */
mkr_status_t mkr_grow_reserve(void **ptr, size_t *cap, size_t need, size_t elem);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_ALLOC_H */
