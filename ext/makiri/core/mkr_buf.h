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
 *   MKR_BUF_HARD_MAX       an absolute ceiling on a buffer's CONTENT length, even
 *                          with an explicit max - the last-resort backstop. The
 *                          ALLOCATION is bounded by it too: geometric growth is
 *                          clamped so cap never exceeds HARD_MAX + 1 (the one NUL
 *                          terminator byte), i.e. it does NOT overshoot to ~2x
 *                          near the limit. Tight, content-scaled bounds still
 *                          belong to the caller (e.g. the XML serializer caps
 *                          itself at a multiple of arena_bytes); this stops total
 *                          runaway.
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

/* ------------------------------------------------------------------------- */
/* mkr_spanbuf_t - a bounded writer over a borrowed, fixed-capacity buffer    */
/* ------------------------------------------------------------------------- */
/*
 * "span" = a non-owning view of a fixed-extent contiguous region; a spanbuf is
 * the bounded *writer* over such a span. The complement to mkr_buf_t: where
 * mkr_buf_t GROWS and OWNS its malloc storage, a spanbuf BORROWS a fixed buffer
 * owned elsewhere (e.g. an arena cut) and never grows it.
 *
 * The name leads with "borrowed/fixed" deliberately, for safety: the
 * fixed/bounded property is SELF-ENFORCED (a write past `cap` is refused, so
 * misunderstanding it costs at most a truncation, caught by _finish), but the
 * BORROWED property is the caller's responsibility - the type cannot stop you
 * from free()ing `buf` or holding `finish()`'s pointer past the owner's
 * lifetime, and getting that wrong is a use-after-free / double-free. So:
 *   - never free() buf (the owner does, e.g. the arena is freed wholesale);
 *   - finish()'s pointer is valid only while the backing storage lives.
 *
 * Bounds safety is BY CONSTRUCTION (the writer owns the cursor + check, not the
 * caller's per-write guard): an over-long write is refused and latches `ok` to
 * false (sticky). The caller's only duty is one check at the end (via _finish or
 * the public `ok` field), failing closed rather than using a truncated buffer.
 * This is the sanctioned way to hand-fill a raw region; see mkr_xml_arena_spanbuf
 * for the arena adapter.
 */
typedef struct {
    char  *buf;   /* borrowed; the owner keeps it alive - do NOT free. NULL => never ok. */
    size_t cap;   /* capacity in bytes (fixed) */
    size_t pos;   /* bytes written so far (always <= cap) */
    bool   ok;    /* false once a write would overflow, or buf was NULL */
} mkr_spanbuf_t;

/* Wrap [buf, buf+cap) for bounded writing. buf == NULL yields a permanently
 * not-ok writer (every write a no-op), so an upstream allocation failure flows
 * straight through without a separate guard at each call site. */
static inline mkr_spanbuf_t
mkr_spanbuf(char *buf, size_t cap)
{
    return (mkr_spanbuf_t){ .buf = buf, .cap = cap, .pos = 0, .ok = (buf != NULL) };
}

/* Append one byte; refuse (latch ok=false) if it would exceed cap. */
static inline void
mkr_spanbuf_putc(mkr_spanbuf_t *b, char c)
{
    if (!b->ok) return;
    if (b->pos >= b->cap) { b->ok = false; return; }
    b->buf[b->pos++] = c;
}

/* Append n bytes; refuse (latch ok=false) if they would exceed cap. n == 0 is a
 * no-op. pos <= cap is the invariant (a refused write never advances pos), so
 * cap - pos cannot underflow; the pos > cap arm is belt-and-suspenders. */
static inline void
mkr_spanbuf_write(mkr_spanbuf_t *b, const void *src, size_t n)
{
    if (!b->ok || n == 0) return;
    if (b->pos > b->cap || n > b->cap - b->pos) { b->ok = false; return; }
    memcpy(b->buf + b->pos, src, n);
    b->pos += n;
}

/* The filled prefix [buf, buf+pos), or NULL if any write was refused (or buf was
 * NULL); on a non-NULL return the length is `b->pos`. Forces the caller through a
 * single fail-closed check instead of trusting the writes individually. */
static inline const char *
mkr_spanbuf_finish(const mkr_spanbuf_t *b)
{
    return b->ok ? b->buf : NULL;
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_BUF_H */
