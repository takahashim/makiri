#include "bridge.h"

mkr_verified_text_t
mkr_verified_text_from_view(mkr_ruby_borrowed_text_t v)
{
    /* The one sanctioned mint of mkr_verified_text_t: v has already passed the
     * strict text contract (mkr_ruby_verified_text / mkr_ruby_try_verified_text). */
    return (mkr_verified_text_t){ v.ptr, v.len };
}
