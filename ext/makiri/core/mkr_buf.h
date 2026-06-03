#ifndef MAKIRI_CORE_MKR_BUF_H
#define MAKIRI_CORE_MKR_BUF_H

/*
 * mkr_buf_t — an owned, growable, optionally capped byte buffer, kept
 * NUL-terminated. Built on the fail-closed allocators in mkr_alloc.h.
 * (mkr_core.h is a thin umbrella over mkr_alloc.h + mkr_text.h + this.)
 */

#include "mkr_alloc.h"

#ifdef __cplusplus
extern "C" {
#endif

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

/* Append n bytes. Fails closed: MKR_ERR_INVALID if n > 0 but bytes == NULL,
 * MKR_ERR_LIMIT if it would exceed max, MKR_ERR_OOM on overflow or allocation
 * failure (the buffer is left intact in every failure case). n == 0 is a no-op. */
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

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_BUF_H */
