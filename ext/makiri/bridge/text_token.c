#include "bridge.h"

mkr_valid_text_t
mkr_text_from_view(mkr_ruby_borrowed_text_t v)
{
    /* The one sanctioned mint of mkr_valid_text_t: v has already passed the
     * strict text contract (mkr_ruby_checked_text / mkr_ruby_engine_string_view). */
    return (mkr_valid_text_t){ v.ptr, v.len };
}
