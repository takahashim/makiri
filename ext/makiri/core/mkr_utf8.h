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

#ifdef __cplusplus
extern "C" {
#endif

bool mkr_utf8_valid(const unsigned char *src, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_UTF8_H */
