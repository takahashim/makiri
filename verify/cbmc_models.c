/* Plain-C bodies for libc functions CBMC's built-in library does not model on
 * this platform (a bodyless callee havocs its return value, turning every
 * downstream pointer check into a false failure). Compiled ONLY into the CBMC
 * runs, never into the smoke/selftest/fuzzer builds (those link the real libc).
 * Each body is the obvious reference implementation, so proving against it is
 * proving against the function's contract. */
#ifdef __CPROVER__

#include <stddef.h>
#include <stdlib.h> /* the SDK's strtod declaration carries an asm alias
                     * (_strtod on macOS); defining without it leaves the
                     * callee bodyless under its aliased name */
#include <string.h>

void *
memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;
    for (size_t i = 0; i < n; ++i) {
        if (p[i] == (unsigned char)c) return (void *)(p + i);
    }
    return NULL;
}

/* strtod, modeled as "libc conversion unavailable" (no bytes consumed). This
 * is a deliberate choice, not a shortcut: correctly-rounded decimal parsing is
 * libc territory we trust rather than verify (like memcpy), and this model
 * steers every caller down its own fallback - for mkr_xpath_number_from_extent
 * that means the isolating-reparse and hand-assembly paths, i.e. exactly OUR
 * code, get proven on all inputs. */
double
strtod(const char *nptr, char **endptr)
{
    if (endptr != NULL) *endptr = (char *)nptr;
    return 0.0;
}

#endif /* __CPROVER__ */
