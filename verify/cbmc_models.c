/* Plain-C bodies for libc functions CBMC's built-in library does not model on
 * this platform (a bodyless callee havocs its return value, turning every
 * downstream pointer check into a false failure). Compiled ONLY into the CBMC
 * runs, never into the smoke/selftest/fuzzer builds (those link the real libc).
 * Each body is the obvious reference implementation, so proving against it is
 * proving against the function's contract. */
#ifdef __CPROVER__

#include <stddef.h>

void *
memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;
    for (size_t i = 0; i < n; ++i) {
        if (p[i] == (unsigned char)c) return (void *)(p + i);
    }
    return NULL;
}

#endif /* __CPROVER__ */
