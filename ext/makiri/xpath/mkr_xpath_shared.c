/* mkr_xpath_shared.c — the representation-independent engine primitives,
 * compiled once.
 *
 * Includes mkr_xpath_internal.h WITHOUT a prelude, so MKR_DOM_NODE stays the
 * neutral `void` default: every function here moves node pointers but never
 * dereferences one, so void* is exact and the object code is shared by the HTML
 * and XML instances (and the driver/parser). See mkr_xpath_shared_body.h. */
#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "mkr_xpath_shared_body.h"
