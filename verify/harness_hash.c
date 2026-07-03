/* CBMC harness: the power-of-two table sizer (core/mkr_hash.h), over ALL
 * size_t inputs (the loop only shifts, so unlike the mul/div guards this
 * closes at full width).
 *
 * mkr_pow2_ceil feeds the open-addressing hash tables; sizing below the
 * element count would make linear probing never find a free slot, so the
 * properties are: on success the result is a power of two, covers n, and is
 * the SMALLEST such power (no oversized tables); failure happens exactly when
 * no power of two >= n fits in size_t.
 */
#include "verify.h"
#include "core/mkr_hash.h"

#define TOP_POW2 ((size_t)1 << (sizeof(size_t) * 8 - 1))

int
main(void)
{
    size_t n = nondet_size_t(), out = 0;
    if (mkr_pow2_ceil(n, &out)) {
        VERIFY_ASSERT(out != 0 && (out & (out - 1)) == 0, "pow2_ceil: a power of two");
        VERIFY_ASSERT(out >= n, "pow2_ceil: covers n");
        VERIFY_ASSERT(out == 1 || (out >> 1) < n, "pow2_ceil: the smallest such power");
    } else {
        VERIFY_ASSERT(n > TOP_POW2, "pow2_ceil: fails only when no power of two fits");
    }
    return 0;
}
