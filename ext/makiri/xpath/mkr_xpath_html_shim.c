/* mkr_xpath_html_shim.c - what the Rust HTML backend cannot reach on its own.
 *
 * Compiled only under MAKIRI_RUST_XPATH_HTML. The Rust backend reads Lexbor's
 * node / element / attr fields directly (they are small, stable structs, and
 * the offsets are checked below), but three operations stay here:
 *
 *   - two reach through lxb_dom_document_t for a field. That struct is large,
 *     carries function-pointer and enum members, and is NOT ours - the Lexbor
 *     pin moves (see CLAUDE.md). Replicating it to read two pointers would be
 *     the most fragile part of the port for the least benefit, and neither is
 *     on a per-node path.
 *   - the text append owns a Lexbor allocation for the length of the call, and
 *     keeping the malloc and the free in one function is what makes that
 *     obviously balanced.
 *
 * The offset check is the other half. Rust's `#[repr(C)]` views of the three
 * node structs would not fail to build if Lexbor reordered a field - they would
 * read the wrong offsets, which is a silent wrong answer. So Rust reports what
 * it believes the layout is and this compares it with the real `offsetof`
 * before anything can run a query.
 */
#ifdef MAKIRI_RUST_XPATH_HTML

#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <lexbor/dom/dom.h>
#include <lexbor/ns/ns.h>
#include <lexbor/tag/tag.h>

#include <stdio.h>
#include <stdlib.h>

/* Borrowed namespace-URI bytes for a node, or NULL with *len 0 when it has
 * none. Mirrors mkr_html_node_ns_uri in mkr_xpath_node_access_html.h. */
const char *
mkr_html_ns_uri(const lxb_dom_node_t *node, const lxb_dom_document_t *doc, size_t *len)
{
    *len = 0;
    if (node == NULL || node->ns == LXB_NS__UNDEF || doc == NULL || doc->ns == NULL) {
        return NULL;
    }
    return (const char *)lxb_ns_by_id(doc->ns, node->ns, len);
}

/* Resolve a tag name to a Lexbor tag id for the //tag index fast path, or
 * LXB_TAG__UNDEF. */
uintptr_t
mkr_html_tag_id_by_name(const lxb_dom_document_t *doc, const char *p, size_t len)
{
    if (doc == NULL || doc->tags == NULL || p == NULL || len == 0) {
        return LXB_TAG__UNDEF;
    }
    return lxb_tag_id_by_name(doc->tags, (const lxb_char_t *)p, len);
}

/* Append a node's own text content to +buf+, returning an mkr_status_t.
 * Lexbor builds the content on demand and hands back an allocation, so the
 * append and the free live together here. */
int
mkr_html_append_own_text(lxb_dom_node_t *node, mkr_buf_t *buf)
{
    size_t tlen = 0;
    lxb_char_t *t = lxb_dom_node_text_content(node, &tlen);
    if (t == NULL) {
        return MKR_OK;
    }
    mkr_status_t st = mkr_buf_append(buf, t, tlen);
    lxb_dom_document_destroy_text(node->owner_document, t);
    return (int)st;
}

/* The end of Lexbor's static tag-id range, exported rather than restated on the
 * Rust side: it is generated from Lexbor's tag table and moves with the pin. */
const size_t mkr_html_tag_last_entry = (size_t)LXB_TAG__LAST_ENTRY;

/* ---- the layout cross-check ---- */

/* Implemented in Rust (rust/src/xpath/html_abi.rs); fills up to `cap` values
 * and returns how many it has. */
size_t mkr_xpath_rs_html_layout(size_t *out, size_t cap);

void
mkr_xpath_rs_html_check(void)
{
    /* The same order as the array in html_abi.rs. */
    const struct { const char *what; size_t value; } expect[] = {
        { "sizeof(lxb_dom_node_t)",       sizeof(lxb_dom_node_t)                       },
        { "node.ns",                      offsetof(lxb_dom_node_t, ns)                 },
        { "node.owner_document",          offsetof(lxb_dom_node_t, owner_document)     },
        { "node.next",                    offsetof(lxb_dom_node_t, next)               },
        { "node.prev",                    offsetof(lxb_dom_node_t, prev)               },
        { "node.parent",                  offsetof(lxb_dom_node_t, parent)             },
        { "node.first_child",             offsetof(lxb_dom_node_t, first_child)        },
        { "node.last_child",              offsetof(lxb_dom_node_t, last_child)         },
        { "node.type",                    offsetof(lxb_dom_node_t, type)               },
        { "sizeof(lxb_dom_element_t)",    sizeof(lxb_dom_element_t)                    },
        { "element.first_attr",           offsetof(lxb_dom_element_t, first_attr)      },
        { "sizeof(lxb_dom_attr_t)",       sizeof(lxb_dom_attr_t)                       },
        { "attr.next",                    offsetof(lxb_dom_attr_t, next)               },
        /* The engine casts a node handle to an element or attr handle, which is
         * only sound while the node sits first in both. */
        { "element.node offset (0)",      offsetof(lxb_dom_element_t, node)            },
        { "attr.node offset (0)",         offsetof(lxb_dom_attr_t, node)               },
        /* Constants the Rust side restates so its hot comparisons stay
         * immediate values; they move with the Lexbor pin like the offsets. */
        { "LXB_NS__UNDEF",                (size_t)LXB_NS__UNDEF                        },
        { "LXB_NS_HTML",                  (size_t)LXB_NS_HTML                          },
        { "LXB_TAG__UNDEF",               (size_t)LXB_TAG__UNDEF                       },
    };
    const size_t n = sizeof(expect) / sizeof(expect[0]);

    size_t got[sizeof(expect) / sizeof(expect[0])] = {0};
    size_t reported = mkr_xpath_rs_html_layout(got, n);
    if (reported != n) {
        fprintf(stderr, "makiri: Rust HTML backend reports %zu layout facts, C checks %zu\n",
                reported, n);
        abort();
    }
    for (size_t i = 0; i < n; i++) {
        if (got[i] != expect[i].value) {
            fprintf(stderr, "makiri: Lexbor layout drift in %s: C %zu, Rust %zu\n",
                    expect[i].what, expect[i].value, got[i]);
            abort();
        }
    }
}

#endif /* MAKIRI_RUST_XPATH_HTML */
