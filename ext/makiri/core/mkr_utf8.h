#ifndef MAKIRI_CORE_MKR_UTF8_H
#define MAKIRI_CORE_MKR_UTF8_H

/*
 * mkr_utf8_valid - the ONE pure-C UTF-8 validator (Ruby-free, allocation-free).
 *
 * Validates [src, src+len) against the Unicode "well-formed UTF-8 byte
 * sequences" table (RFC 3629 / WHATWG): rejects bad continuation bytes,
 * overlong forms, surrogates (U+D800..U+DFFF), code points above U+10FFFF, and
 * an incomplete trailing sequence. Validate-only - it never materialises code
 * points - and rips through ASCII a machine word at a time. NUL bytes are VALID
 * here (U+0000 is well-formed UTF-8); callers that must reject NUL check it
 * separately (memchr).
 *
 * This lives in core so the Ruby bridge (mkr_verify_text - the strict
 * programmatic-input gate) and the HTML input sanitiser (lexbor_compat/
 * utf8_input.c fast path) share a single implementation, and so the bridge's
 * validation never allocates: a borrowed RSTRING pointer must not be held
 * across a Ruby allocation (= GC point), so the validator the bridge runs
 * between taking a borrow and using it has to be allocation-free by
 * construction. (The former implementation built a throwaway Ruby String and
 * asked for its coderange - an allocation inside every borrow.)
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "mkr_span.h"

#ifdef __cplusplus
extern "C" {
#endif

bool mkr_utf8_valid(const unsigned char *src, size_t len);

/* mkr_utf8_decode1 - decode ONE code point from [p, p+len), strictly: rejects
 * truncation, bad continuation bytes, overlong forms, surrogates and values
 * above U+10FFFF. Returns the byte length (1-4) with *cp set, or 0 on any
 * violation (including len == 0) - fail closed, never read past the bound.
 * The ONE strict decoder, shared by the XML tokenizer's name/Char scanning and
 * the XPath lexer (each formerly carried its own equivalent copy). */
int mkr_utf8_decode1(const unsigned char *p, size_t len, uint32_t *cp);

/* Span form: decode the code point at the span's cursor (without consuming -
 * the caller mkr_span_skip()s the returned length). 0 at end-of-span. */
static inline int
mkr_utf8_decode1_span(const mkr_span_t *s, uint32_t *cp)
{
    return mkr_utf8_decode1((const unsigned char *)s->p, mkr_span_left(s), cp);
}

/* mkr_utf8_count_chars - count Unicode code points in [ptr, ptr+len): every
 * byte that is NOT a 0x80..0xBF continuation byte starts a new code point.
 * Length-bounded (does not rely on a NUL terminator); ptr may be NULL when
 * len == 0. Used where XPath measures string length / offsets in characters. */
static inline size_t
mkr_utf8_count_chars(const char *ptr, size_t len)
{
    size_t n = 0;
    for (size_t i = 0; i < len; ++i) {
        if (((unsigned char)ptr[i] & 0xC0) != 0x80) ++n;
    }
    return n;
}

/* mkr_utf8_advance_chars - byte offset within [ptr, ptr+len) after advancing up
 * to nchars UTF-8 characters from the start, clamped at len. A character is its
 * leading byte plus the run of 0x80..0xBF continuation bytes that follow;
 * advancing stops at len even mid-sequence. Length-bounded (no NUL reliance).
 * Returns len when nchars exceeds the available character count. */
static inline size_t
mkr_utf8_advance_chars(const char *ptr, size_t len, size_t nchars)
{
    size_t i = 0;
    while (nchars > 0 && i < len) {
        ++i;
        while (i < len && ((unsigned char)ptr[i] & 0xC0) == 0x80) ++i;
        --nchars;
    }
    return i;
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_UTF8_H */
