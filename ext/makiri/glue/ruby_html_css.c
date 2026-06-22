#include "glue.h"

#include <lexbor/css/css.h>
#include <lexbor/selectors/selectors.h>

/*
 * CSS selector queries, delegated to Lexbor's lxb_selectors engine.
 *
 *   Node#css(selector)    -> NodeSet (descendants matching, document order)
 *   Node#at_css(selector) -> first matching descendant, or nil
 *
 * The Lexbor CSS engine (selector parser + its arena + the traversal engine) is
 * built once and reused for every query. CSS evaluation always holds the GVL (it
 * never releases it), so all queries are serialized and a single process-global
 * engine is safe with no locking. Creating and tearing the engine down per call
 * - four create/init/destroy triples - dominated a cheap query like
 * at_css('#id') (the match is found almost immediately, so setup IS the cost);
 * reusing it closes the gap to nokolexbor, which caches the same objects in
 * thread-local storage. Between calls only the parsed selector list's arena is
 * reset (lxb_css_memory_clean) and the parser is returned to its CLEAN stage
 * (lxb_css_parser_clean) - both preserve the memory/selectors objects set once;
 * the selectors parse-state is auto-cleaned by the parser and the traversal
 * engine self-cleans after each find/match. A malformed selector raises
 * Makiri::CSS::SyntaxError.
 */

/* Process-global CSS engine, created lazily and kept for the process lifetime
 * (one small allocation). parser->memory / parser->selectors are set once so the
 * parser reuses the same selector arena + parse state across calls. */
static lxb_css_memory_t    *g_css_mem;
static lxb_css_parser_t    *g_css_parser;
static lxb_css_selectors_t *g_css_sel;
static lxb_selectors_t     *g_selectors;
static int                  g_css_ready;

/* Compiled-selector cache: a Ruby Hash mapping a selector String to the parsed
 * lxb_css_selector_list_t (stored as an Integer pointer). Parsing a selector is
 * the dominant cost when the same selectors are queried over and over (a
 * querySelector-heavy SPA fires tens of thousands of identical queries), so the
 * compiled list is reused instead of re-parsed. The parsed lists live in the
 * shared g_css_mem arena, which is therefore NOT cleaned per call (the original
 * code reset it after every query); to bound growth, when the cache fills we drop
 * every compiled list at once (clean the arena + clear the Hash) and start over. */
static VALUE                g_css_cache;
#define MKR_CSS_CACHE_CAP   256

/* Adaptive caching. Holding many distinct compiled lists in the shared arena
 * makes each new parse slower (a bigger arena = more allocator work), so a flood
 * of one-off selectors -- e.g. getElementById on unique React `useId` ids, which
 * are never requeried -- turned the cache into a net loss vs the original
 * parse+clean (measured ~22% slower per call). Track the cache hit rate over a
 * window and, when it is low, BYPASS the cache: parse + clean per call, exactly
 * as before, so the arena stays small and the worst case is merely "as fast as no
 * cache". Periodically drop back into caching to re-test, so a workload that
 * starts repeating selectors regains the cache. */
static size_t               g_css_win;        /* lookups in the current window */
static size_t               g_css_win_hits;   /* cache hits in the current window */
static int                  g_css_bypass;     /* 1 = parse+clean, no caching */
static size_t               g_css_bypass_runs; /* consecutive bypass windows */
#define MKR_CSS_WIN          1024  /* re-evaluate the hit rate every N lookups */
#define MKR_CSS_MIN_HIT_PCT  15    /* below this hit rate, bypass the cache */
#define MKR_CSS_RETEST_GAP   32    /* re-test caching every N bypass windows */

/* Build the shared engine on first use; raises Makiri::Error on init failure
 * (leaving the globals unset, so a later call retries). */
static void
mkr_css_engine_init(void)
{
    if (g_css_ready) {
        return;
    }

    lxb_css_memory_t    *mem       = lxb_css_memory_create();
    lxb_css_parser_t    *parser    = lxb_css_parser_create();
    lxb_css_selectors_t *css_sel   = lxb_css_selectors_create();
    lxb_selectors_t     *selectors = lxb_selectors_create();

    int ok = (mem != NULL && parser != NULL && css_sel != NULL && selectors != NULL)
          && (lxb_css_memory_init(mem, 128) == LXB_STATUS_OK)
          && (lxb_css_parser_init(parser, NULL) == LXB_STATUS_OK)
          && (lxb_css_selectors_init(css_sel) == LXB_STATUS_OK)
          && (lxb_selectors_init(selectors) == LXB_STATUS_OK);

    if (!ok) {
        if (selectors != NULL) lxb_selectors_destroy(selectors, true);
        if (parser != NULL)    lxb_css_parser_destroy(parser, true);
        if (mem != NULL)       lxb_css_memory_destroy(mem, true);
        if (css_sel != NULL)   lxb_css_selectors_destroy(css_sel, true);
        rb_raise(mkr_eError, "failed to initialise CSS selector engine");
    }

    lxb_css_parser_memory_set(parser, mem);
    lxb_css_parser_selectors_set(parser, css_sel);

    g_css_mem    = mem;
    g_css_parser = parser;
    g_css_sel    = css_sel;
    g_selectors  = selectors;
    g_css_ready  = 1;
}

/* find: the callback runs inside Lexbor's traversal frames, so it must NOT
 * raise - a longjmp would abort lxb_selectors_find mid-walk AND skip the
 * post-run reset in mkr_with_compiled_selector, leaving the process-global
 * parser/memory/selectors dirty for every later query. So matches are collected
 * into a plain C array here (no Ruby at all); the node cap and a growth failure
 * each latch a flag and STOP, and the caller raises only after the traversal
 * has unwound normally and the engine was reset. */
typedef struct {
    lxb_dom_node_t **nodes;     /* malloc-backed; the caller owns + frees it */
    size_t           count;
    size_t           cap;
    lxb_dom_node_t  *root;      /* excluded from results: css is descendant-only */
    int              overflow;
    int              oom;
} mkr_css_ctx_t;

static lxb_status_t
mkr_css_find_cb(lxb_dom_node_t *node, lxb_css_selector_specificity_t spec,
                void *ctx_)
{
    (void)spec;
    mkr_css_ctx_t *c = (mkr_css_ctx_t *)ctx_;

    if (node == c->root) {
        return LXB_STATUS_OK; /* descendant-only, like Nokogiri's node.css */
    }
    if (c->count >= MKR_NODE_SET_MAX) {
        c->overflow = 1;
        return LXB_STATUS_STOP; /* fail closed without raising mid-traversal */
    }
    if (mkr_grow_reserve((void **)&c->nodes, &c->cap, c->count + 1,
                         sizeof(*c->nodes)) != MKR_OK) {
        c->oom = 1;
        return LXB_STATUS_STOP; /* ditto: report OOM after the walk unwinds */
    }
    c->nodes[c->count++] = node;
    return LXB_STATUS_OK;
}

/* Move the collected matches into the NodeSet under rb_ensure, so the C array
 * cannot leak even if a push raises (NoMemoryError). */
typedef struct {
    VALUE          set;
    mkr_css_ctx_t *ctx;
} mkr_css_fill_t;

static VALUE
mkr_css_fill_set(VALUE p)
{
    mkr_css_fill_t *f = (mkr_css_fill_t *)p;
    for (size_t i = 0; i < f->ctx->count; i++) {
        mkr_node_set_push(f->set, (mkr_raw_node_t *)f->ctx->nodes[i]);
    }
    return Qnil;
}

static VALUE
mkr_css_free_nodes(VALUE p)
{
    free(((mkr_css_ctx_t *)p)->nodes);
    return Qnil;
}

/* at_css: capture the first matching descendant and stop. Avoids materialising a
 * NodeSet (and a Ruby #first dispatch) for the one node the caller wants. */
typedef struct {
    lxb_dom_node_t *root;       /* excluded: descendant-only */
    lxb_dom_node_t *found;
} mkr_css_first_ctx_t;

static lxb_status_t
mkr_css_first_cb(lxb_dom_node_t *node, lxb_css_selector_specificity_t spec,
                 void *ctx_)
{
    (void)spec;
    mkr_css_first_ctx_t *c = (mkr_css_first_ctx_t *)ctx_;

    if (node == c->root) {
        return LXB_STATUS_OK; /* descendant-only */
    }
    c->found = node;
    return LXB_STATUS_STOP;
}

/* Callback for matches?: signals that the node matched the selector. */
static lxb_status_t
mkr_css_match_cb(lxb_dom_node_t *node, lxb_css_selector_specificity_t spec,
                 void *ctx_)
{
    (void)node; (void)spec;
    *(int *)ctx_ = 1;
    return LXB_STATUS_STOP;
}

/* Parse +rb_selector+ with the shared engine, hand the parsed list to +run+
 * (the actual find / match against +node+), then reset the engine for the next
 * call. Raises Makiri::CSS::SyntaxError on a bad selector; any result-specific
 * limits are checked by the caller after return. */
static void
mkr_with_compiled_selector(VALUE rb_selector, lxb_dom_node_t *node,
                           lxb_status_t (*run)(lxb_selectors_t *, lxb_dom_node_t *,
                                               lxb_css_selector_list_t *, void *),
                           void *u)
{
    mkr_ruby_borrowed_text_t sv = mkr_ruby_verified_text(rb_selector, "CSS selector");

    mkr_css_engine_init(); /* raises on init failure */

    if (g_css_cache == 0) {
        g_css_cache = rb_hash_new();
        rb_gc_register_address(&g_css_cache);
    }

    /* Adaptive window: every MKR_CSS_WIN lookups, decide whether caching is
     * paying off. A low hit rate (mostly one-off selectors) means the cache is
     * only growing the arena and slowing parses, so bypass it; periodically
     * re-test so a workload that becomes selector-repetitive regains the cache. */
    if (++g_css_win > MKR_CSS_WIN) {
        if (!g_css_bypass) {
            if (g_css_win_hits * 100 < (size_t)MKR_CSS_WIN * MKR_CSS_MIN_HIT_PCT) {
                g_css_bypass = 1;
                g_css_bypass_runs = 0;
                lxb_css_memory_clean(g_css_mem); /* drop the cached lists' arena */
                rb_hash_clear(g_css_cache);
            }
        } else if (++g_css_bypass_runs >= MKR_CSS_RETEST_GAP) {
            g_css_bypass = 0; /* re-test caching over the next window */
        }
        g_css_win = 1;
        g_css_win_hits = 0;
    }

    if (!g_css_bypass) {
        /* Reuse the compiled selector when we have already parsed this string. */
        VALUE cached = rb_hash_lookup(g_css_cache, sv.value);
        if (!NIL_P(cached)) {
            lxb_css_selector_list_t *list =
                (lxb_css_selector_list_t *)(intptr_t)NUM2LL(cached);
            g_css_win_hits++;
            (void)run(g_selectors, node, list, u);
            /* The traversal engine self-cleans; the cached list + arena persist. */
            RB_GC_GUARD(sv.value);
            return;
        }

        /* Cache miss. Bound the cache before parsing: when full, drop every
         * compiled list at once by cleaning the shared arena, then start fresh —
         * so the new list is parsed into the now-empty arena (cleaning AFTER
         * parsing would invalidate it). */
        if (RHASH_SIZE(g_css_cache) >= MKR_CSS_CACHE_CAP) {
            lxb_css_memory_clean(g_css_mem);
            rb_hash_clear(g_css_cache);
        }

        lxb_css_selector_list_t *list =
            lxb_css_selectors_parse(g_css_parser, (const lxb_char_t *)sv.ptr, sv.len);
        int syntax_error = (list == NULL || g_css_parser->status != LXB_STATUS_OK);

        /* Return the parser to its CLEAN stage; do NOT clean the memory arena —
         * the freshly parsed list lives there and we keep it. */
        lxb_css_parser_clean(g_css_parser);

        if (syntax_error) {
            rb_raise(mkr_eCSSSyntaxError, "invalid CSS selector: %" PRIsVALUE, sv.value);
        }

        /* Cache the compiled list (the Hash dups + freezes the String key), then
         * run. Store the pointer as a Ruby Integer; the list outlives the call. */
        rb_hash_aset(g_css_cache, sv.value, LL2NUM((long long)(intptr_t)list));
        (void)run(g_selectors, node, list, u);
        RB_GC_GUARD(sv.value);
        return;
    }

    /* Bypass mode: parse + clean per call (the original behavior), so the arena
     * stays small and a one-off-selector flood is no slower than no cache. */
    lxb_css_selector_list_t *l0 =
        lxb_css_selectors_parse(g_css_parser, (const lxb_char_t *)sv.ptr, sv.len);
    int err0 = (l0 == NULL || g_css_parser->status != LXB_STATUS_OK);
    if (!err0) {
        (void)run(g_selectors, node, l0, u);
    }
    lxb_css_memory_clean(g_css_mem);
    lxb_css_parser_clean(g_css_parser);
    if (err0) {
        rb_raise(mkr_eCSSSyntaxError, "invalid CSS selector: %" PRIsVALUE, sv.value);
    }
    RB_GC_GUARD(sv.value);
}

/* find: collect descendants matching the selector (MATCH_FIRST dedups a node
 * that matches several selectors in a comma list). */
static lxb_status_t
mkr_run_find(lxb_selectors_t *selectors, lxb_dom_node_t *root,
             lxb_css_selector_list_t *list, void *u)
{
    lxb_selectors_opt_set(selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
    return lxb_selectors_find(selectors, root, list, mkr_css_find_cb, u);
}

/* find_first: stop at the first matching descendant (for at_css). */
static lxb_status_t
mkr_run_find_first(lxb_selectors_t *selectors, lxb_dom_node_t *root,
                   lxb_css_selector_list_t *list, void *u)
{
    lxb_selectors_opt_set(selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
    return lxb_selectors_find(selectors, root, list, mkr_css_first_cb, u);
}

/* match_node: does THIS node match? */
static lxb_status_t
mkr_run_match(lxb_selectors_t *selectors, lxb_dom_node_t *node,
              lxb_css_selector_list_t *list, void *u)
{
    return lxb_selectors_match_node(selectors, node, list, mkr_css_match_cb, u);
}

/* Node#css: collect every matching descendant into a NodeSet (document order).
 * Raises Makiri::CSS::SyntaxError on a bad selector, Makiri::Error on an
 * over-large result. */
static VALUE
mkr_node_css(VALUE self, VALUE rb_selector)
{
    lxb_dom_node_t *root = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);

    /* A syntax error raises inside mkr_with_compiled_selector BEFORE the find
     * runs, so ctx.nodes is still NULL there - nothing leaks on that path. */
    mkr_css_ctx_t ctx = { .nodes = NULL, .count = 0, .cap = 0,
                          .root = root, .overflow = 0, .oom = 0 };
    mkr_with_compiled_selector(rb_selector, root, mkr_run_find, &ctx);

    if (ctx.overflow || ctx.oom) {
        free(ctx.nodes);
        if (ctx.overflow) {
            rb_raise(mkr_eError, "CSS result set exceeded the node limit (%u)",
                     MKR_NODE_SET_MAX);
        }
        rb_raise(mkr_eError, "out of memory collecting CSS results");
    }

    VALUE set = mkr_node_set_new(document);
    mkr_css_fill_t fill = { set, &ctx };
    rb_ensure(mkr_css_fill_set, (VALUE)&fill, mkr_css_free_nodes, (VALUE)&ctx);
    return set;
}

/* Node#at_css: the first matching descendant, or nil. */
static VALUE
mkr_node_at_css(VALUE self, VALUE rb_selector)
{
    lxb_dom_node_t *root = mkr_html_node_unwrap(self);

    mkr_css_first_ctx_t ctx = { .root = root, .found = NULL };
    mkr_with_compiled_selector(rb_selector, root, mkr_run_find_first, &ctx);

    return ctx.found != NULL
        ? mkr_wrap_html_node(ctx.found, mkr_node_document(self))
        : Qnil;
}

/* Node#matches?(selector): does THIS node match the CSS selector? (Like
 * Nokogiri - tested against the node itself, not its descendants.) A malformed
 * selector raises Makiri::CSS::SyntaxError. */
static VALUE
mkr_node_matches(VALUE self, VALUE rb_selector)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    int matched = 0;
    mkr_with_compiled_selector(rb_selector, node, mkr_run_match, &matched);
    return matched ? Qtrue : Qfalse;
}

void
mkr_init_css(void)
{
    rb_define_method(mkr_mHtmlNodeMethods, "css",     mkr_node_css,     1);
    rb_define_method(mkr_mHtmlNodeMethods, "at_css",  mkr_node_at_css,  1);
    rb_define_method(mkr_mHtmlNodeMethods, "matches?", mkr_node_matches, 1);
}
