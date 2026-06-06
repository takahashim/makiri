#ifndef MAKIRI_CORE_MKR_HASH_H
#define MAKIRI_CORE_MKR_HASH_H

/*
 * Primitives for the pointer-keyed open-addressing hash tables used by the
 * indexes (attr->owner, text-index, the XPath string-value cache, the
 * document-order index): a pointer hash and a power-of-two table sizer.
 * Ruby-free. (mkr_core.h is a thin umbrella over the core headers.)
 */

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Mix a pointer into a well-distributed 64-bit hash (the MurmurHash3 fmix64
 * finalizer). Aligned pointers carry little low-bit entropy, so spread them
 * before masking with a power-of-two table size. The one definition shared by
 * the pointer-keyed indexes (attr->owner, text-index, ...); mask the result
 * with `& (cap - 1)` at the call site. */
static inline uint64_t
mkr_ptr_hash(const void *p)
{
    uint64_t h = (uint64_t)(uintptr_t)p;
    h ^= h >> 33;
    h *= 0xff51afd7ed558ccdULL;
    h ^= h >> 33;
    h *= 0xc4ceb9fe1a85ec53ULL;
    h ^= h >> 33;
    return h;
}

/* Smallest power of two >= n, into *out. Returns false on overflow (no power of
 * two >= n fits in size_t) so the caller fails closed rather than sizing a
 * power-of-two hash table below the element count it must hold - which would
 * never find a free slot under linear probing. Shared by the pointer-keyed
 * indexes (attr->owner, text-index). */
static inline bool
mkr_pow2_ceil(size_t n, size_t *out)
{
    size_t p = 1;
    while (p < n) {
        size_t np = p << 1;
        if (np <= p) return false; /* overflow */
        p = np;
    }
    *out = p;
    return true;
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_HASH_H */
