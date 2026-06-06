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

typedef struct {
    VALUE           set;
    lxb_dom_node_t *root;       /* excluded from results: css is descendant-only */
    size_t          count;
    int             overflow;
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

    mkr_node_set_push(c->set, (mkr_raw_node_t *)node);
    c->count++;
    return LXB_STATUS_OK;
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

    lxb_css_selector_list_t *list =
        lxb_css_selectors_parse(g_css_parser, (const lxb_char_t *)sv.ptr, sv.len);

    int syntax_error = (list == NULL || g_css_parser->status != LXB_STATUS_OK);
    if (!syntax_error) {
        (void)run(g_selectors, node, list, u);
    }

    /* Reset the shared engine for the next query: drop the parsed list's arena
     * allocations and return the parser to its CLEAN stage. Both preserve the
     * memory/selectors objects we set once; the traversal engine self-cleans
     * after find/match. */
    lxb_css_memory_clean(g_css_mem);
    lxb_css_parser_clean(g_css_parser);

    if (syntax_error) {
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
    VALUE set = mkr_node_set_new(document);

    mkr_css_ctx_t ctx = { .set = set, .root = root, .count = 0, .overflow = 0 };
    mkr_with_compiled_selector(rb_selector, root, mkr_run_find, &ctx);

    if (ctx.overflow) {
        rb_raise(mkr_eError, "CSS result set exceeded the node limit (%u)",
                 MKR_NODE_SET_MAX);
    }
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
