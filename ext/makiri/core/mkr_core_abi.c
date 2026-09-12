/* mkr_core_abi.c - build-time constants published for the Rust side.
 *
 * The core limits are `-D`-overridable macros (see mkr_buf.h), so their VALUE
 * belongs to the build, not to the source. A Rust `const` restating one would
 * be right for the default build and silently wrong for a
 * `-DMKR_BUF_HARD_MAX=<bytes>` one - two ceilings in one extension, which is
 * the failure this file exists to remove. Exporting them from a translation
 * unit the C preprocessor has already seen makes the build the single source.
 *
 * Nothing here is replaceable: this file has no RustPorts row and is compiled
 * into every configuration, precisely because it is what the Rust half reads
 * the configuration from. Its only content is constants.
 *
 * The same pattern already appears in xpath/mkr_xpath_html_shim.c, which
 * exports LXB_TAG__LAST_ENTRY for the element index.
 */
#include "mkr_buf.h"

/* The absolute ceiling on a buffer's CONTENT length. */
const size_t mkr_buf_hard_max = (size_t)MKR_BUF_HARD_MAX;

/* The ceiling applied when a buffer was initialised with max == 0. Not
 * "unbounded" - that is the whole point of having a default. */
const size_t mkr_buf_default_limit = (size_t)MKR_BUF_DEFAULT_LIMIT;
