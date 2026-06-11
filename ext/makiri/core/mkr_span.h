#ifndef MAKIRI_CORE_MKR_SPAN_H
#define MAKIRI_CORE_MKR_SPAN_H

/*
 * mkr_span_t - a bounded READER over a borrowed byte region: the read twin of
 * mkr_spanbuf_t (mkr_buf.h), completing the structural-safety model:
 *
 *               allocate            write              read
 *   structure   checked wrappers    mkr_spanbuf_t      mkr_span_t (this)
 *   enforced    lint (direct_alloc) (arena's only way) lint (raw_scan_call /
 *                                                            raw_cursor_member)
 *
 * Byte-scanning parsers used to guard every read by convention ("check
 * p < end, then *p") - correct at every current site, but a single forgotten
 * check is invisible to the compiler. A span owns the cursor AND the bound:
 * an out-of-bounds read is not an overrun but a -1 / false / clamp, so the
 * unchecked-read bug class is structurally impossible inside a converted TU.
 * The lint (script/check_c_safety.rb) bans the raw scanning primitives in the
 * converted parser TUs, turning the convention into a machine-enforced rule.
 *
 * Every helper is a static inline whose bound check compiles to the same
 * single compare the hand-written guard used - performance-neutral.
 *
 * Like a spanbuf, the BORROW is the caller's responsibility: the span cannot
 * stop you from outliving the buffer it views (input lifetime is handled at
 * the bridge: parse entries copy, borrowed slices never cross a GC point).
 * mkr_span_mark/mkr_span_since exist to CAPTURE slices (pointer arithmetic,
 * never a dereference); reading a captured slice goes back through a span or
 * an audited core primitive.
 */

#include <stdbool.h>
#include <stddef.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    const char *p;    /* cursor (always <= end) */
    const char *end;  /* one past the last readable byte */
} mkr_span_t;

/* Wrap [ptr, ptr+len) for bounded reading. ptr == NULL yields the empty span
 * (every read -1 / false), so an absent input flows through without a guard.
 * The NULL case is normalized to a VALID empty address (not p = end = NULL):
 * the helpers do pointer subtraction and relational compares, which C defines
 * only within one object - NULL - NULL / NULL < NULL is formally UB - so the
 * normalization here keeps every helper unconditional AND well-defined. */
static inline mkr_span_t
mkr_span(const char *ptr, size_t len)
{
    if (ptr == NULL) { ptr = ""; len = 0; }
    mkr_span_t s;
    s.p   = ptr;
    s.end = ptr + len;
    return s;
}

/* Bytes remaining. */
static inline size_t
mkr_span_left(const mkr_span_t *s)
{
    return (size_t)(s->end - s->p);
}

/* The byte at the cursor as 0..255, or -1 at end-of-span. */
static inline int
mkr_span_peek(const mkr_span_t *s)
{
    return s->p < s->end ? (unsigned char)*s->p : -1;
}

/* The byte at cursor+i (bounded lookahead), or -1 past the end. */
static inline int
mkr_span_at(const mkr_span_t *s, size_t i)
{
    return i < mkr_span_left(s) ? (unsigned char)s->p[i] : -1;
}

/* Consume and return the byte at the cursor, or -1 at end-of-span. */
static inline int
mkr_span_take(mkr_span_t *s)
{
    return s->p < s->end ? (unsigned char)*s->p++ : -1;
}

/* Advance up to n bytes (clamped at the end - never past it). */
static inline void
mkr_span_skip(mkr_span_t *s, size_t n)
{
    size_t left = mkr_span_left(s);
    s->p += (n <= left) ? n : left;
}

/* True if the remaining input begins with the n-byte literal. n == 0 is true
 * regardless of lit (even NULL - aligned with mkr_bytes_eq; also keeps the
 * memcmp away from a possibly-NULL lit, which is formally UB even for 0). */
static inline bool
mkr_span_starts(const mkr_span_t *s, const char *lit, size_t n)
{
    if (n == 0) return true;
    return mkr_span_left(s) >= n && memcmp(s->p, lit, n) == 0;
}

/* Find byte c in the remaining input; true + its offset from the cursor in
 * *idx, or false if absent (idx untouched). */
static inline bool
mkr_span_find(const mkr_span_t *s, char c, size_t *idx)
{
    const char *hit = (const char *)memchr(s->p, c, mkr_span_left(s));
    if (hit == NULL) return false;
    *idx = (size_t)(hit - s->p);
    return true;
}

/* The remaining input from cursor+off onward, as a fresh sub-span (clamped at
 * the end). For bounded lookahead scans that must not consume the parent. */
static inline mkr_span_t
mkr_span_tail(const mkr_span_t *s, size_t off)
{
    mkr_span_t t = *s;
    mkr_span_skip(&t, off);
    return t;
}

/* The cursor position, for capturing a slice start (an address, NEVER read
 * through directly - pair with mkr_span_since and hand the slice to a span or
 * an audited primitive). */
static inline const char *
mkr_span_mark(const mkr_span_t *s)
{
    return s->p;
}

/* Bytes consumed since +mark+ (a prior mkr_span_mark of the SAME span). */
static inline size_t
mkr_span_since(const mkr_span_t *s, const char *mark)
{
    return (size_t)(s->p - mark);
}

/* Length-checked slice equality (the audited replacement for an open-coded
 * memcmp over two captured slices). Zero-length slices are equal regardless
 * of pointers (a NULL "" never gets dereferenced). */
static inline bool
mkr_bytes_eq(const void *a, size_t alen, const void *b, size_t blen)
{
    return alen == blen && (alen == 0 || memcmp(a, b, alen) == 0);
}

/* Substring search: find the first occurrence of [needle, needle+needle_len)
 * within [hay, hay+hay_len). On a hit returns true and writes the byte offset to
 * *idx; on a miss returns false and leaves *idx untouched. The audited
 * replacement for an open-coded substring memcmp (a scan, so it lives in core).
 * Boundary behavior mirrors mkr_bytes_eq's: lengths are the truth, pointers are
 * never dereferenced at length 0 -
 *   - an empty needle (needle_len == 0) matches at offset 0 in ANY haystack,
 *     including an empty or NULL "" one (the substring analogue of two empty
 *     slices comparing equal);
 *   - a needle longer than the haystack never matches, so a NULL/empty haystack
 *     misses every non-empty needle without a read. */
static inline bool
mkr_bytes_find(const void *hay, size_t hay_len,
               const void *needle, size_t needle_len, size_t *idx)
{
    if (needle_len == 0) { *idx = 0; return true; }
    if (needle_len > hay_len) return false;
    const char *h = (const char *)hay;
    const char *n = (const char *)needle;
    size_t last = hay_len - needle_len;
    for (size_t i = 0; i <= last; ++i) {
        if (h[i] == n[0] && memcmp(h + i, n, needle_len) == 0) {
            *idx = i;
            return true;
        }
    }
    return false;
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_SPAN_H */
