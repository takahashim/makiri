#ifndef MAKIRI_CORE_MKR_CORE_H
#define MAKIRI_CORE_MKR_CORE_H

/*
 * Umbrella over the Ruby-free core primitives. Include this to pull in the
 * whole core layer; or include just the specific header you need:
 *
 *   mkr_alloc.h  fail-closed size arithmetic + allocators (the foundation)
 *   mkr_hash.h   pointer hash + power-of-two sizer (pointer-keyed index tables)
 *   mkr_text.h   string-type lattice (owned/borrowed/verified text + bytes)
 *   mkr_buf.h    mkr_buf_t (growable, capped byte buffer)
 *
 * NOTHING here touches Ruby - exception mapping happens at the glue boundary.
 */

#include "mkr_alloc.h"
#include "mkr_hash.h"
#include "mkr_text.h"
#include "mkr_buf.h"

#endif /* MAKIRI_CORE_MKR_CORE_H */
