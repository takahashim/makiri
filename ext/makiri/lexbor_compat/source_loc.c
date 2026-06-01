#include "compat.h"
#include "compat_internal.h"

#include <lexbor/html/parser.h>
#include <lexbor/html/tokenizer.h>
#include <lexbor/tag/const.h>

#include <stdint.h>
#include <stdlib.h>

/*
 * Source location tracking. Lexbor does not record where in the input a node
 * came from. We stay on vanilla Lexbor and reconstruct it from the tokenizer
 * instead:
 *
 *   1. mkr_pos_recorder_t chains the tokenizer's token-done callback and logs
 *      (tag_id, byte offset) for every element start-tag, in token order.
 *   2. After the tree is built, mkr_pos_assign_to_dom walks the DOM pre-order
 *      and matches each element to the next compatible recorded token,
 *      stamping the byte offset into node->user.
 *   3. mkr_lines_t maps a byte offset to a 1-based line for Node#line.
 *
 * Precision is ~the HTML5 tree-construction reorderings (foster parenting,
 * adoption agency) away from perfect; on a mismatch we leave the node
 * unstamped (line => nil) rather than reporting a wrong location.
 */

/* ------------------------------------------------------------------ */
/* line table                                                         */
/* ------------------------------------------------------------------ */

typedef struct {
    size_t *starts; /* starts[i] = byte offset of line (i+1); starts[0] == 0 */
    size_t  count;  /* number of lines (>= 1) */
} mkr_lines_t;

void *
mkr_lines_build(const lxb_char_t *src, size_t len)
{
    if (src == NULL) {
        len = 0;
    }

    /* One pass to count newlines, so the array is sized once. */
    size_t nl = 0;
    for (size_t i = 0; i < len; i++) {
        if (src[i] == '\n') {
            nl++;
        }
    }

    mkr_lines_t *t = malloc(sizeof(*t));
    if (t == NULL) {
        return NULL;
    }
    t->count  = nl + 1;
    t->starts = malloc(t->count * sizeof(*t->starts));
    if (t->starts == NULL) {
        free(t);
        return NULL;
    }

    size_t line = 0;
    t->starts[line++] = 0;
    for (size_t i = 0; i < len; i++) {
        if (src[i] == '\n') {
            t->starts[line++] = i + 1;
        }
    }
    return t;
}

void
mkr_lines_free(void *lines)
{
    if (lines == NULL) {
        return;
    }
    mkr_lines_t *t = lines;
    free(t->starts);
    free(t);
}

/* Largest line whose start offset is <= offset, 1-based. */
static size_t
mkr_lines_lookup(const mkr_lines_t *t, size_t offset)
{
    size_t lo = 0, hi = t->count; /* find last starts[i] <= offset */
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (t->starts[mid] <= offset) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    return lo; /* lo is the count of starts <= offset == 1-based line */
}

/* ------------------------------------------------------------------ */
/* token position recorder                                            */
/* ------------------------------------------------------------------ */

/* Defensive cap so a pathological input cannot make the transient recorder
 * grow without bound. On overflow we stop recording and skip assignment
 * entirely, so locations degrade to "unknown" rather than to wrong values. */
#define MKR_POS_MAX_TOKENS (10u * 1000u * 1000u)

typedef struct {
    lxb_tag_id_t tag_id;
    size_t       offset;
} mkr_pos_entry_t;

struct mkr_pos_recorder_s {
    mkr_pos_entry_t *items;
    size_t           count;
    size_t           cap;
    const lxb_char_t *first; /* start of the input buffer (for offsets) */
    int              overflow;

    /* The parser's own token-done callback, which actually builds the tree. */
    lxb_html_tokenizer_token_f orig;
    void                      *orig_ctx;
};

mkr_pos_recorder_t *
mkr_pos_recorder_create(const lxb_char_t *src)
{
    mkr_pos_recorder_t *rec = calloc(1, sizeof(*rec));
    if (rec == NULL) {
        return NULL;
    }
    rec->first = src;
    return rec;
}

void
mkr_pos_recorder_destroy(mkr_pos_recorder_t *rec)
{
    if (rec == NULL) {
        return;
    }
    free(rec->items);
    free(rec);
}

void
mkr_pos_recorder_set_delegate(mkr_pos_recorder_t *rec,
                              lxb_html_tokenizer_token_f orig, void *orig_ctx)
{
    if (rec == NULL) {
        return;
    }
    rec->orig     = orig;
    rec->orig_ctx = orig_ctx;
}

static void
mkr_pos_record(mkr_pos_recorder_t *rec, lxb_html_token_t *token)
{
    /* Element start-tags only: skip the special tag ids (text, comment,
     * doctype, document, eof) and end-tags. CLOSE_SELF (void/self-closing
     * start tags such as <br/>) keeps bit CLOSE clear, so it is recorded. */
    if (token->tag_id <= LXB_TAG__EM_DOCTYPE
        || token->begin == NULL
        || (token->type & LXB_HTML_TOKEN_TYPE_CLOSE) != 0) {
        return;
    }

    if (rec->count == rec->cap) {
        size_t ncap = (rec->cap == 0) ? 64 : rec->cap * 2;
        if (ncap > MKR_POS_MAX_TOKENS) {
            rec->overflow = 1;
            return;
        }
        mkr_pos_entry_t *p = realloc(rec->items, ncap * sizeof(*p));
        if (p == NULL) {
            rec->overflow = 1; /* fail closed: stop recording */
            return;
        }
        rec->items = p;
        rec->cap   = ncap;
    }

    rec->items[rec->count].tag_id = token->tag_id;
    rec->items[rec->count].offset = (size_t)(token->begin - rec->first);
    rec->count++;
}

lxb_html_token_t *
mkr_pos_token_cb(lxb_html_tokenizer_t *tkz, lxb_html_token_t *token, void *ctx)
{
    mkr_pos_recorder_t *rec = ctx;
    if (!rec->overflow) {
        mkr_pos_record(rec, token);
    }
    /* Always delegate so the parser still builds the tree. */
    return rec->orig(tkz, token, rec->orig_ctx);
}

/* ------------------------------------------------------------------ */
/* assignment                                                         */
/* ------------------------------------------------------------------ */

/* Pre-order successor (mkr_dom_preorder_next) lives in compat_internal.h. */

/* How far ahead of the cursor we'll look for a tag-id match. Bounds the damage
 * from a dropped/reordered token: an unmatched element stays unstamped instead
 * of grabbing a far-away token's offset. */
#define MKR_POS_LOOKAHEAD 64

void
mkr_pos_assign_to_dom(mkr_pos_recorder_t *rec, lxb_dom_node_t *root)
{
    if (rec == NULL || rec->overflow || root == NULL) {
        return;
    }

    size_t cursor = 0;
    for (lxb_dom_node_t *node = root; node != NULL;
         node = mkr_dom_preorder_next(node, root)) {
        if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            continue;
        }
        if (cursor >= rec->count) {
            break;
        }
        lxb_tag_id_t tid = (lxb_tag_id_t)node->local_name;
        size_t limit = cursor + MKR_POS_LOOKAHEAD;
        if (limit > rec->count) {
            limit = rec->count;
        }
        for (size_t j = cursor; j < limit; j++) {
            if (rec->items[j].tag_id == tid) {
                /* +1 so a genuine offset of 0 is distinguishable from unset. */
                node->user = (void *)(uintptr_t)(rec->items[j].offset + 1);
                cursor = j + 1;
                break;
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* line lookup for Ruby Node#line                                     */
/* ------------------------------------------------------------------ */

size_t
mkr_parsed_node_line(mkr_parsed_t *p, lxb_dom_node_t *node)
{
    if (p == NULL || node == NULL || node->user == NULL
        || p->newline_idx == NULL) {
        return 0;
    }
    size_t offset = (size_t)(uintptr_t)node->user - 1;
    return mkr_lines_lookup((const mkr_lines_t *)p->newline_idx, offset);
}
