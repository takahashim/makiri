#ifndef MAKIRI_VERIFY_H
#define MAKIRI_VERIFY_H

/*
 * Shared shim for the verification harnesses (docs/formal_verification.ja.md).
 *
 * Each harness builds two ways:
 *   - under CBMC (`make cbmc-*`): nondet_* are CBMC's nondeterministic inputs,
 *     VERIFY_ASSUME/ASSERT map to __CPROVER_assume/assert, and the checks run
 *     over ALL inputs within the harness bounds;
 *   - under a plain cc (`make smoke`): the SAME file links into a normal
 *     executable with pseudo-random inputs. This is the link check docs §7
 *     asks for (CBMC treats a missing function body as a nondet return and
 *     never errors, so the linker is what catches a wrong source list).
 */

#include <stddef.h>
#include <stdint.h>

#ifdef __CPROVER__

size_t        nondet_size_t(void);
unsigned char nondet_uchar(void);
/* Function calls are side effects, so callers compute `bool ok = f(...)` and
 * assume/assert the result, never VERIFY_ASSUME(f(...)). */
#define VERIFY_ASSUME(cond)      __CPROVER_assume(cond)
#define VERIFY_ASSERT(cond, msg) __CPROVER_assert((cond), msg)

#else /* plain cc smoke build */

#include <assert.h>
#include <stdio.h>

static uint64_t verify_seed_ = 0x9E3779B97F4A7C15ull;
static inline uint64_t
verify_next_(void)
{
    verify_seed_ ^= verify_seed_ << 13;
    verify_seed_ ^= verify_seed_ >> 7;
    verify_seed_ ^= verify_seed_ << 17;
    return verify_seed_;
}
/* Small-biased so the smoke run actually reaches the interesting branches
 * (a uniform size_t would trip the first ASSUME and exit immediately). */
static inline size_t        nondet_size_t(void) { return (size_t)(verify_next_() % 64); }
static inline unsigned char nondet_uchar(void)  { return (unsigned char)verify_next_(); }

/* An unmet assumption just ends the (single) smoke iteration. */
#define VERIFY_ASSUME(cond)      do { if (!(cond)) return 0; } while (0)
#define VERIFY_ASSERT(cond, msg) assert((cond) && msg)

#endif

#endif /* MAKIRI_VERIFY_H */
