/* mkr_xpath_rs_check.c - the Rust front end's layout cross-check.
 *
 * Compiled only under MAKIRI_RUST_XPATH=1 (extconf defines it alongside the
 * cargo `xpath` feature). The Rust parser writes C-layout AST nodes through
 * pointers, so a field added or reordered on one side and not the other would
 * not fail to build - it would write the wrong offsets at runtime, which is a
 * silent wrong answer, the one outcome the engine must never produce.
 *
 * So Rust reports what it thinks the layouts are (mkr_xpath_rs_sizes, in
 * rust/src/xpath/abi.rs) and this runs at load time, before anything parses.
 * A disagreement aborts: the alternative is a corrupt AST. Sizes alone do not
 * prove the offsets match, but they catch every change that adds, removes or
 * retypes a field, which is what drift looks like in practice.
 */
#ifdef MAKIRI_RUST_XPATH

#include "mkr_xpath_internal.h"

#include <stdio.h>
#include <stdlib.h>

/* Implemented in Rust; fills up to `cap` sizes and returns how many it has. */
size_t mkr_xpath_rs_sizes(size_t *out, size_t cap);

void
mkr_xpath_rs_check(void)
{
    /* The same order as the array in abi.rs. */
    const struct { const char *name; size_t size; } expect[] = {
        { "mkr_node_t",          sizeof(mkr_node_t)              },
        { "mkr_step_t",          sizeof(mkr_step_t)              },
        { "mkr_nodetest_t",      sizeof(mkr_nodetest_t)          },
        { "mkr_val_t",           sizeof(mkr_val_t)               },
        { "mkr_node_s::u",       sizeof(((mkr_node_t *)0)->u)    },
        { "mkr_xpath_limits_t",  sizeof(mkr_xpath_limits_t)      },
        { "mkr_xpath_error_t",   sizeof(mkr_xpath_error_t)       },
        { "mkr_verified_text_t", sizeof(mkr_verified_text_t)     },
    };
    const size_t n = sizeof(expect) / sizeof(expect[0]);

    size_t got[sizeof(expect) / sizeof(expect[0])] = {0};
    size_t reported = mkr_xpath_rs_sizes(got, n);
    if (reported != n) {
        fprintf(stderr, "makiri: Rust XPath front end reports %zu layouts, C checks %zu\n",
                reported, n);
        abort();
    }
    for (size_t i = 0; i < n; i++) {
        if (got[i] != expect[i].size) {
            fprintf(stderr, "makiri: XPath layout drift in %s: C %zu, Rust %zu\n",
                    expect[i].name, expect[i].size, got[i]);
            abort();
        }
    }
}

#endif /* MAKIRI_RUST_XPATH */
