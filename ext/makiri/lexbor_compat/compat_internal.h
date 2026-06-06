#ifndef MAKIRI_COMPAT_INTERNAL_H
#define MAKIRI_COMPAT_INTERNAL_H

/* Low-level helpers shared across the extension's C translation units (the
 * lexbor_compat layer and the Ruby↔C glue) but not part of the compat public
 * API in compat.h. */

#include <lexbor/dom/dom.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Next node of a pre-order (document-order) traversal bounded to `root`'s
 * subtree, walked via parent pointers - no recursion and no heap stack, so an
 * adversarially deep tree cannot exhaust the C stack (DoS-safe). Returns the
 * first child, then the next sibling, climbing back toward `root` (never above
 * it) when a branch is exhausted; NULL after the last node in the subtree.
 *
 * Single copy on purpose: this DoS-avoiding invariant is relied on by both the
 * attr/element index build and the source-location walk, and must not drift. */
static inline lxb_dom_node_t *
mkr_dom_preorder_next(lxb_dom_node_t *node, lxb_dom_node_t *root)
{
    if (node->first_child != NULL) {
        return node->first_child;
    }
    while (node != root && node->next == NULL) {
        node = node->parent;
    }
    if (node == root) {
        return NULL;
    }
    return node->next;
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_COMPAT_INTERNAL_H */
