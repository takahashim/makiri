#include "glue.h"

#include <lexbor/css/css.h>
#include <lexbor/css/stylesheet.h>
#include <lexbor/css/rule.h>
#include <lexbor/css/at_rule.h>
#include <lexbor/css/property.h>
#include <lexbor/css/selectors/selector.h>

#include "../core/mkr_core.h"

/*
 * Makiri::Lexbor::CSS.parse_stylesheet(text) -> Array
 *
 * A deliberately THIN binding over Lexbor's CSS stylesheet parser. It returns
 * the parsed rules as plain Ruby primitives (Array / Hash / Symbol / String /
 * Integer) - no Makiri class - so the abstraction seam lives in the caller
 * (e.g. dommy's internal/css/parser.rb), not here. Shape:
 *
 *   [ { type: :style,
 *       selectors:    [ { text: "div.a", specificity: [a, b, c] }, ... ],
 *       declarations: [ { name: "display", value: "flex", important: false }, ... ] },
 *     { type: :media,
 *       condition: "(min-width: 600px)",
 *       rules:     [ ...same style/media hashes, nested... ] },
 *     ... ]   # source order
 *
 * Error recovery follows css-syntax-3 (and dommy's contract): a malformed
 * declaration is dropped, a rule with an invalid selector list surfaces as a
 * LXB_CSS_RULE_BAD_STYLE and is SKIPPED, an unknown at-rule is skipped. So a
 * broken stylesheet never raises a syntax error - it yields the valid rules
 * only. A hard parser failure (OOM) raises Makiri::Error.
 *
 * Unlike Node#css's engine, the stylesheet parser is created and destroyed per
 * call: stylesheet parsing is rare (once per <style>), not a hot path, so a
 * fresh parser+stylesheet avoids any process-global mutable state and keeps the
 * lifetime trivially fail-closed (freed under rb_ensure on any Ruby raise).
 */

/* Bound on @media nesting depth: fail closed instead of unbounded recursion on
 * a pathologically nested stylesheet. */
#define MKR_LEXBOR_CSS_MAX_DEPTH 64u

/* ----- serialization into an owned, growable buffer ----------------------- */

typedef struct {
    mkr_buf_t buf;
    int       oom;   /* set when an append fails; serialization then stops */
} mkr_css_ser_t;

static lxb_status_t
mkr_css_ser_cb(const lxb_char_t *data, size_t len, void *ctx)
{
    mkr_css_ser_t *s = (mkr_css_ser_t *)ctx;
    if (mkr_buf_append(&s->buf, data, len) != MKR_OK) {
        s->oom = 1;
        return LXB_STATUS_ERROR; /* stop the serializer */
    }
    return LXB_STATUS_OK;
}

/* Drive +serialize+ (one of Lexbor's *_serialize callbacks) into a fresh UTF-8
 * Ruby String. +scratch+ is reused across calls (its content is reset each
 * time); the scratch buffer is owned by the caller and freed in the cleanup
 * path, so a raise from rb_utf8_str_new cannot leak it. Raises Makiri::Error on
 * an allocation failure inside the serializer. */
typedef lxb_status_t (*mkr_css_ser_fn)(void *subject, lexbor_serialize_cb_f cb,
                                       void *ctx);

static VALUE
mkr_css_serialize_to_str(mkr_buf_t *scratch, mkr_css_ser_fn fn, void *subject)
{
    mkr_css_ser_t s = { .buf = *scratch, .oom = 0 };
    s.buf.len = 0; /* reuse the storage, drop previous content */

    lxb_status_t st = fn(subject, mkr_css_ser_cb, &s);

    *scratch = s.buf; /* hand back the (possibly grown) storage */

    if (s.oom) {
        rb_raise(mkr_eError, "out of memory serializing CSS");
    }
    if (st != LXB_STATUS_OK) {
        rb_raise(mkr_eError, "failed to serialize CSS");
    }
    return rb_utf8_str_new(scratch->data ? scratch->data : "",
                           (long)scratch->len);
}

/* Adapters matching mkr_css_ser_fn for the specific Lexbor serializers. */
static lxb_status_t
mkr_ser_selector_chain(void *subject, lexbor_serialize_cb_f cb, void *ctx)
{
    /* One comma-branch only: serialize this list's selector chain WITHOUT
     * following list->next (which would re-emit the whole comma list). */
    return lxb_css_selector_serialize_chain((lxb_css_selector_t *)subject, cb, ctx);
}

/* ----- specificity -------------------------------------------------------- */

/* [a, b, c] per Selectors L4 §17 (id, class/attr/pseudo-class, type/pseudo-el).
 * The packed !important / style-attribute flags are never set on a parsed
 * stylesheet rule, so they are intentionally dropped. */
static VALUE
mkr_css_specificity_ary(lxb_css_selector_specificity_t sp)
{
    VALUE a = rb_ary_new_capa(3);
    rb_ary_push(a, INT2FIX((int)lxb_css_selector_sp_a(sp)));
    rb_ary_push(a, INT2FIX((int)lxb_css_selector_sp_b(sp)));
    rb_ary_push(a, INT2FIX((int)lxb_css_selector_sp_c(sp)));
    return a;
}

/* ----- rule conversion ---------------------------------------------------- */

/* Shared state threaded through the conversion: the original input bytes (for
 * @media prelude slicing) and the reusable serialization scratch buffer. */
typedef struct {
    const char *css;       /* borrowed input bytes (kept alive by the caller) */
    size_t      css_len;
    mkr_buf_t   scratch;
} mkr_css_conv_t;

static VALUE mkr_css_rules_to_ary(mkr_css_conv_t *c, lxb_css_rule_t *first,
                                  unsigned depth);

/* lxb_css_property_serialize / _name take (style, type, cb, ctx); wrap them to
 * the (subject, cb, ctx) shape by carrying the property pointer + type. */
typedef struct {
    const void *style;
    uintptr_t   type;
} mkr_css_prop_subject_t;

static lxb_status_t
mkr_ser_prop_value(void *subject, lexbor_serialize_cb_f cb, void *ctx)
{
    mkr_css_prop_subject_t *p = (mkr_css_prop_subject_t *)subject;
    return lxb_css_property_serialize(p->style, p->type, cb, ctx);
}

static lxb_status_t
mkr_ser_prop_name(void *subject, lexbor_serialize_cb_f cb, void *ctx)
{
    mkr_css_prop_subject_t *p = (mkr_css_prop_subject_t *)subject;
    return lxb_css_property_serialize_name(p->style, p->type, cb, ctx);
}

static VALUE
mkr_css_declarations_ary(mkr_css_conv_t *c, lxb_css_rule_declaration_list_t *list)
{
    VALUE arr = rb_ary_new();
    if (list == NULL) {
        return arr;
    }

    for (lxb_css_rule_t *r = list->first; r != NULL; r = r->next) {
        if (r->type != LXB_CSS_RULE_DECLARATION) {
            continue;
        }
        lxb_css_rule_declaration_t *decl = lxb_css_rule_declaration(r);
        mkr_css_prop_subject_t subj = { .style = decl->u.user, .type = decl->type };

        VALUE name  = mkr_css_serialize_to_str(&c->scratch, mkr_ser_prop_name,  &subj);
        VALUE value = mkr_css_serialize_to_str(&c->scratch, mkr_ser_prop_value, &subj);

        VALUE h = rb_hash_new();
        rb_hash_aset(h, ID2SYM(rb_intern("name")),  name);
        rb_hash_aset(h, ID2SYM(rb_intern("value")), value);
        rb_hash_aset(h, ID2SYM(rb_intern("important")),
                     decl->important ? Qtrue : Qfalse);
        rb_ary_push(arr, h);
    }
    return arr;
}

/* [ { text:, specificity: [a,b,c] }, ... ] - one entry per comma branch. */
static VALUE
mkr_css_selectors_ary(mkr_css_conv_t *c, lxb_css_selector_list_t *sel)
{
    VALUE arr = rb_ary_new();
    for (lxb_css_selector_list_t *l = sel; l != NULL; l = l->next) {
        VALUE text = mkr_css_serialize_to_str(&c->scratch, mkr_ser_selector_chain,
                                              l->first);
        VALUE h = rb_hash_new();
        rb_hash_aset(h, ID2SYM(rb_intern("text")), text);
        rb_hash_aset(h, ID2SYM(rb_intern("specificity")),
                     mkr_css_specificity_ary(l->specificity));
        rb_ary_push(arr, h);
    }
    return arr;
}

/* { type: :style, selectors: [...], declarations: [...] } */
static VALUE
mkr_css_style_hash(mkr_css_conv_t *c, lxb_css_rule_style_t *style)
{
    VALUE h = rb_hash_new();
    rb_hash_aset(h, ID2SYM(rb_intern("type")), ID2SYM(rb_intern("style")));
    rb_hash_aset(h, ID2SYM(rb_intern("selectors")),
                 mkr_css_selectors_ary(c, style->selector));
    rb_hash_aset(h, ID2SYM(rb_intern("declarations")),
                 mkr_css_declarations_ary(c, style->declarations));
    return h;
}

/* Slice the @media condition text out of the original input using the at-rule's
 * recorded prelude byte offsets, trimming ASCII whitespace. Lexbor does not
 * retain the media query on the parsed media struct, but it keeps the offsets
 * on the rule, and we still hold the input bytes. Returns "" if the offsets are
 * unusable (fail closed - never a wrong slice). */
static VALUE
mkr_css_media_condition(mkr_css_conv_t *c, lxb_css_rule_at_t *at)
{
    size_t b = at->prelude_begin;
    size_t e = at->prelude_end;
    if (b > e || e > c->css_len) {
        return rb_utf8_str_new("", 0);
    }
    const char *p = c->css + b;
    size_t n = e - b;
    while (n > 0 && (unsigned char)p[0] <= ' ')   { p++; n--; }
    while (n > 0 && (unsigned char)p[n - 1] <= ' ') { n--; }
    return rb_utf8_str_new(p, (long)n);
}

/* { type: :media, condition: "...", rules: [...] } or Qnil if not @media. */
static VALUE
mkr_css_at_rule_hash(mkr_css_conv_t *c, lxb_css_rule_at_t *at, unsigned depth)
{
    if (at->type != LXB_CSS_AT_RULE_MEDIA) {
        return Qnil; /* @font-face/@namespace/unknown: skipped for now */
    }

    lxb_css_at_rule_media_t *media = at->u.media;
    lxb_css_rule_t *block_first =
        (media != NULL && media->block != NULL) ? media->block->first : NULL;

    VALUE h = rb_hash_new();
    rb_hash_aset(h, ID2SYM(rb_intern("type")), ID2SYM(rb_intern("media")));
    rb_hash_aset(h, ID2SYM(rb_intern("condition")), mkr_css_media_condition(c, at));
    rb_hash_aset(h, ID2SYM(rb_intern("rules")),
                 mkr_css_rules_to_ary(c, block_first, depth + 1));
    return h;
}

/* Convert a sibling chain of rules (first..NULL) into an Array, skipping rules
 * we do not surface (bad-style, non-media at-rules). */
static VALUE
mkr_css_rules_to_ary(mkr_css_conv_t *c, lxb_css_rule_t *first, unsigned depth)
{
    if (depth > MKR_LEXBOR_CSS_MAX_DEPTH) {
        rb_raise(mkr_eError, "CSS @media nesting too deep (max %u)",
                 MKR_LEXBOR_CSS_MAX_DEPTH);
    }

    VALUE arr = rb_ary_new();
    for (lxb_css_rule_t *r = first; r != NULL; r = r->next) {
        VALUE entry = Qnil;
        switch (r->type) {
            case LXB_CSS_RULE_STYLE:
                entry = mkr_css_style_hash(c, lxb_css_rule_style(r));
                break;
            case LXB_CSS_RULE_AT_RULE:
                entry = mkr_css_at_rule_hash(c, lxb_css_rule_at(r), depth);
                break;
            default:
                /* LXB_CSS_RULE_BAD_STYLE and anything else: error recovery
                 * dropped it; do not surface it. */
                break;
        }
        if (!NIL_P(entry)) {
            rb_ary_push(arr, entry);
        }
    }
    return arr;
}

/* ----- entry point -------------------------------------------------------- */

typedef struct {
    lxb_css_parser_t     *parser;
    lxb_css_stylesheet_t *sst;
    mkr_css_conv_t        conv;
} mkr_css_parse_state_t;

static VALUE
mkr_css_parse_body(VALUE arg)
{
    mkr_css_parse_state_t *s = (mkr_css_parse_state_t *)arg;

    lxb_status_t st = lxb_css_stylesheet_parse(
        s->sst, s->parser,
        (const lxb_char_t *)s->conv.css, s->conv.css_len);

    /* Lexbor recovers from CSS syntax errors and still returns OK; a non-OK
     * status here is a hard failure (e.g. OOM). */
    if (st != LXB_STATUS_OK || s->sst->root == NULL) {
        if (st == LXB_STATUS_OK && s->sst->root == NULL) {
            return rb_ary_new(); /* empty / whitespace-only stylesheet */
        }
        rb_raise(mkr_eError, "failed to parse CSS stylesheet");
    }

    lxb_css_rule_t *first = lxb_css_rule_list(s->sst->root)->first;
    return mkr_css_rules_to_ary(&s->conv, first, 0);
}

static VALUE
mkr_css_parse_cleanup(VALUE arg)
{
    mkr_css_parse_state_t *s = (mkr_css_parse_state_t *)arg;
    if (s->sst != NULL) {
        (void)lxb_css_stylesheet_destroy(s->sst, true);
        s->sst = NULL;
    }
    if (s->parser != NULL) {
        (void)lxb_css_parser_destroy(s->parser, true);
        s->parser = NULL;
    }
    mkr_buf_free(&s->conv.scratch);
    return Qnil;
}

static VALUE
mkr_lexbor_css_parse_stylesheet(VALUE self, VALUE rb_text)
{
    (void)self;
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, "CSS stylesheet");

    mkr_css_parse_state_t s = {
        .parser = NULL,
        .sst    = NULL,
        .conv   = { .css = tv.ptr, .css_len = tv.len },
    };
    mkr_buf_init(&s.conv.scratch, 0);

    s.parser = lxb_css_parser_create();
    s.sst    = lxb_css_stylesheet_create(NULL);
    if (s.parser == NULL || s.sst == NULL
        || lxb_css_parser_init(s.parser, NULL) != LXB_STATUS_OK)
    {
        mkr_css_parse_cleanup((VALUE)&s);
        rb_raise(mkr_eError, "failed to initialise CSS parser");
    }

    VALUE result = rb_ensure(mkr_css_parse_body, (VALUE)&s,
                             mkr_css_parse_cleanup, (VALUE)&s);
    RB_GC_GUARD(tv.value);
    return result;
}

void
mkr_init_lexbor_css(void)
{
    VALUE mod = rb_define_module_under(mkr_mLexbor, "CSS");
    rb_define_module_function(mod, "parse_stylesheet",
                              mkr_lexbor_css_parse_stylesheet, 1);
}
