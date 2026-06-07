#include "mkr_buf.h"

mkr_status_t
mkr_buf_append(mkr_buf_t *b, const void *bytes, size_t n)
{
    if (n == 0) {
        return MKR_OK;
    }
    if (bytes == NULL) {
        return MKR_ERR_INVALID; /* fail closed: nonzero length with no source */
    }
    size_t need;
    if (!mkr_size_add(b->len, n, &need)) {
        return MKR_ERR_OOM;
    }
    /* max == 0 is NOT unbounded: it falls back to the conservative default
     * ceiling, so a caller that never set a cap still fails closed. Either way the
     * absolute hard ceiling clamps it, so no buffer can exhaust memory. */
    size_t soft  = (b->max != 0) ? b->max : MKR_BUF_DEFAULT_LIMIT;
    size_t limit = (soft < MKR_BUF_HARD_MAX) ? soft : MKR_BUF_HARD_MAX;
    if (need > limit) {
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
        /* Geometric growth can overshoot to ~2x need_term; clamp the ALLOCATION
         * to the same ceiling as the content (limit, plus one byte for the NUL),
         * so cap never runs to ~2x MKR_BUF_HARD_MAX near the limit. Safe: this
         * append already passed need <= limit, so need_term <= limit + 1 and the
         * clamp never drops new_cap below what this append needs. (If limit + 1
         * overflows - only a pathological -DMKR_BUF_HARD_MAX=SIZE_MAX - skip it.) */
        size_t cap_ceiling;
        if (mkr_size_add(limit, 1, &cap_ceiling) && new_cap > cap_ceiling) {
            new_cap = cap_ceiling;
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

mkr_status_t
mkr_buf_reserve(mkr_buf_t *b, size_t n)
{
    /* Pre-allocate capacity for n bytes so a known-size fill does not realloc on
     * every geometric step (the serializer reserves ~the output size up front).
     * Best-effort: never grow past the buffer's own cap, and a later append still
     * fails closed if the real output exceeds it. */
    size_t soft  = (b->max != 0) ? b->max : MKR_BUF_DEFAULT_LIMIT;
    size_t limit = (soft < MKR_BUF_HARD_MAX) ? soft : MKR_BUF_HARD_MAX;
    if (n > limit) {
        n = limit;
    }
    size_t need_term; /* room for the NUL terminator too */
    if (!mkr_size_add(n, 1, &need_term)) {
        return MKR_ERR_OOM;
    }
    if (need_term <= b->cap) {
        return MKR_OK; /* already have room */
    }
    char *p = realloc(b->data, need_term);
    if (p == NULL) {
        return MKR_ERR_OOM;
    }
    b->data = p;
    b->cap  = need_term;
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
