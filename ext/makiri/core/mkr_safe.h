#ifndef MAKIRI_CORE_MKR_SAFE_H
#define MAKIRI_CORE_MKR_SAFE_H

/*
 * Umbrella over the Ruby-free safe-core headers. Kept so existing
 * `#include "core/mkr_safe.h"` sites keep working; new code may include just the
 * specific header it needs:
 *
 *   mkr_alloc.h  fail-closed size arithmetic + allocators (the foundation)
 *   mkr_text.h   string-type lattice (owned/borrowed/verified text + bytes)
 *   mkr_buf.h    mkr_buf_t (growable, capped byte buffer)
 *
 * NOTHING here touches Ruby — exception mapping happens at the glue boundary.
 */

#include "mkr_alloc.h"
#include "mkr_text.h"
#include "mkr_buf.h"

#endif /* MAKIRI_CORE_MKR_SAFE_H */
