#ifndef MAKIRI_CORE_MKR_BUF_H
#define MAKIRI_CORE_MKR_BUF_H

/*
 * mkr_buf_t - an owned, growable, optionally capped byte buffer, kept
 * NUL-terminated. Built on the fail-closed allocators in mkr_alloc.h.
 * (mkr_core.h is a thin umbrella over mkr_alloc.h + mkr_text.h + this.)
 */

#include "mkr_alloc.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Memory safety for buffers lives HERE, at the one buffer primitive, not at each
 * call site: "max == 0" can no longer mean "unbounded". Two ceilings bound every
 * mkr_buf so a runaway - a cycle, an unbounded loop, or a caller that forgot to
 * pass a cap - fails closed with MKR_ERR_LIMIT instead of exhausting memory and
 * freezing the machine:
 *
 *   MKR_BUF_DEFAULT_LIMIT  the cap applied when the caller passes max == 0. A
 *                          conservative default (100 MiB): code that did not
 *                          think about a bound gets a tight one for free, and a
 *                          buffer that genuinely needs to be large must opt in
 *                          EXPLICITLY by passing a larger max.
 *   MKR_BUF_HARD_MAX       an absolute ceiling no buffer may exceed, even one
 *                          with an explicit max - the last-resort backstop.
 *                          Tight, content-scaled bounds still belong to the
 *                          caller (e.g. the XML serializer caps itself at a
 *                          multiple of arena_bytes); this stops total runaway.
 *
 * Override either at build time: -DMKR_BUF_DEFAULT_LIMIT=<bytes> / -DMKR_BUF_HARD_MAX=<bytes>. */
#ifndef MKR_BUF_DEFAULT_LIMIT
#define MKR_BUF_DEFAULT_LIMIT ((size_t)100 << 20)   /* 100 MiB */
#endif
#ifndef MKR_BUF_HARD_MAX
#define MKR_BUF_HARD_MAX ((size_t)4 << 30)        /* 4 GiB */
#endif

typedef struct {
    char  *data; /* owned; kept NUL-terminated after any append */
    size_t len;  /* bytes used (excluding the terminator) */
    size_t cap;  /* bytes allocated */
    size_t max;  /* 0 = the conservative MKR_BUF_DEFAULT_LIMIT; else this value -
                  * either way clamped by MKR_BUF_HARD_MAX (past it -> ERR_LIMIT) */
} mkr_buf_t;

/* Initialise an empty buffer. max == 0 applies the conservative default ceiling
 * (MKR_BUF_DEFAULT_LIMIT) - it is NOT unbounded; pass an explicit (larger or
 * smaller) value to opt into a different bound, always under MKR_BUF_HARD_MAX. */
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

/* Pre-allocate capacity for at least n bytes (best-effort, clamped to the
 * buffer's cap), so a fill of known approximate size avoids per-append reallocs.
 * A no-op if the buffer already has room. Returns MKR_ERR_OOM on overflow /
 * allocation failure (the buffer is left intact). */
mkr_status_t mkr_buf_reserve(mkr_buf_t *b, size_t n);

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
