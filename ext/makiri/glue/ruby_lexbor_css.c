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
 *     { type: :bad_style,                 # selector lexbor rejected (e.g. ::before)
 *       selector_text: "p::before",       # raw prelude for the caller to re-validate
 *       declarations:  [ ... ] },
 *     { type: :at_rule,                   # every at-rule, surfaced uniformly
 *       name:    "media",                 # the keyword after `@`
 *       prelude: "(min-width: 600px)",    # text before the block (condition/name/...)
 *       rules:   [ ...same style/at_rule hashes, nested... ] },  # [] for @import etc.
 *     ... ]   # source order
 *
 * Error recovery follows css-syntax-3: a malformed declaration is dropped, an
 * unknown at-rule is skipped. A rule whose selector list lexbor cannot parse
 * surfaces as :bad_style with its raw prelude text (lexbor is stricter/older
 * than Selectors L4, so the caller re-validates with its own selector parser).
 * So a broken stylesheet never raises a syntax error - it yields the parsed
 * rules. A hard parser failure (OOM) raises Makiri::Error.
 *
 * Unlike Node#css's engine, the stylesheet parser is created and destroyed per
 * call: stylesheet parsing is rare (once per <style>), not a hot path, so a
 * fresh parser+stylesheet avoids any process-global mutable state and keeps the
 * lifetime trivially fail-closed (freed under rb_ensure on any Ruby raise).
 */

/* Bound on at-rule nesting depth: fail closed instead of unbounded recursion on
 * a pathologically nested stylesheet. */
#define MKR_LEXBOR_CSS_MAX_DEPTH 64u

/* Interned IDs for the fixed result-hash keys and :type values, cached once in
 * mkr_init_lexbor_css so the conversion loops avoid a per-entry rb_intern hash
 * lookup (ID2SYM at the use sites is a free bit-shift). */
static ID id_type, id_selectors, id_declarations, id_name, id_value,
          id_important, id_text, id_specificity, id_selector_text, id_prelude,
          id_rules;
static ID id_sym_style, id_sym_bad_style, id_sym_at_rule;

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
        rb_hash_aset(h, ID2SYM(id_name),  name);
        rb_hash_aset(h, ID2SYM(id_value), value);
        rb_hash_aset(h, ID2SYM(id_important),
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
        rb_hash_aset(h, ID2SYM(id_text), text);
        rb_hash_aset(h, ID2SYM(id_specificity),
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
    rb_hash_aset(h, ID2SYM(id_type), ID2SYM(id_sym_style));
    rb_hash_aset(h, ID2SYM(id_selectors),
                 mkr_css_selectors_ary(c, style->selector));
    rb_hash_aset(h, ID2SYM(id_declarations),
                 mkr_css_declarations_ary(c, style->declarations));
    return h;
}

/* Slice [begin, end) out of the original input, trimming ASCII whitespace.
 * Lexbor keeps byte offsets (name / prelude) on every at-rule and we still hold
 * the input bytes, so this recovers an at-rule's name and prelude uniformly.
 * Returns "" if the offsets are unusable (fail closed - never a wrong slice). */
static VALUE
mkr_css_slice_trim(mkr_css_conv_t *c, size_t begin, size_t end)
{
    if (begin > end || end > c->css_len) {
        return rb_utf8_str_new("", 0);
    }
    const char *p = c->css + begin;
    size_t n = end - begin;
    while (n > 0 && (unsigned char)p[0] <= ' ')   { p++; n--; }
    while (n > 0 && (unsigned char)p[n - 1] <= ' ') { n--; }
    return rb_utf8_str_new(p, (long)n);
}

/* The at-rule keyword (without `@`). Typed at-rules carry no name field, so map
 * from the type; a custom at-rule (any keyword lexbor has no dedicated parser
 * for - @supports/@layer/@keyframes/...) keeps the verbatim ident in
 * custom->name. The on-rule name_begin offset is unreliable (not reset between
 * sibling rules), so it is intentionally not used. */
static VALUE
mkr_css_at_name(lxb_css_rule_at_t *at)
{
    switch (at->type) {
        case LXB_CSS_AT_RULE_MEDIA:
            return rb_utf8_str_new("media", 5);
        case LXB_CSS_AT_RULE_FONT_FACE:
            return rb_utf8_str_new("font-face", 9);
        case LXB_CSS_AT_RULE_NAMESPACE:
            return rb_utf8_str_new("namespace", 9);
        case LXB_CSS_AT_RULE__CUSTOM: {
            lxb_css_at_rule__custom_t *cu = at->u.custom;
            if (cu != NULL && cu->name.data != NULL) {
                return rb_utf8_str_new((const char *)cu->name.data,
                                       (long)cu->name.length);
            }
            return rb_utf8_str_new("", 0);
        }
        default: /* __UNDEF (malformed) and anything else: unnamed */
            return rb_utf8_str_new("", 0);
    }
}

/* The nested block of any at-rule that has one (@media/@supports/@layer/
 * @keyframes/@font-face/... all parse their `{ ... }` into a rule list at a
 * type-specific union member), or NULL for statement at-rules (@import,
 * @namespace, @charset). */
static lxb_css_rule_list_t *
mkr_css_at_block(lxb_css_rule_at_t *at)
{
    switch (at->type) {
        case LXB_CSS_AT_RULE_MEDIA:
            return (at->u.media != NULL) ? at->u.media->block : NULL;
        case LXB_CSS_AT_RULE_FONT_FACE:
            return (at->u.font_face != NULL) ? at->u.font_face->block : NULL;
        case LXB_CSS_AT_RULE__CUSTOM:
            return (at->u.custom != NULL) ? at->u.custom->block : NULL;
        case LXB_CSS_AT_RULE__UNDEF:
            return (at->u.undef != NULL) ? at->u.undef->block : NULL;
        default: /* @namespace and other statement at-rules: no block */
            return NULL;
    }
}

/* { type: :bad_style, selector_text: "...", declarations: [...] }
 *
 * Lexbor's selectors parser rejects some selectors that Selectors L4 accepts -
 * pseudo-elements (`::before`) most notably - and records them as BAD_STYLE,
 * keeping the raw prelude text and the (still parsed) declaration block. Rather
 * than drop these, surface the raw selector text so the caller can re-validate
 * with its own (newer) selector parser. The caller owns the W3C selector
 * semantics; this binding stays a thin lexbor face. */
static VALUE
mkr_css_bad_style_hash(mkr_css_conv_t *c, lxb_css_rule_bad_style_t *bad)
{
    VALUE h = rb_hash_new();
    const char *txt = (bad->selectors.data != NULL)
                          ? (const char *)bad->selectors.data : "";
    rb_hash_aset(h, ID2SYM(id_type), ID2SYM(id_sym_bad_style));
    rb_hash_aset(h, ID2SYM(id_selector_text),
                 rb_utf8_str_new(txt, (long)bad->selectors.length));
    rb_hash_aset(h, ID2SYM(id_declarations),
                 mkr_css_declarations_ary(c, bad->declarations));
    return h;
}

/* { type: :at_rule, name: "media", prelude: "(min-width: 600px)", rules: [...] }
 *
 * Every at-rule is surfaced uniformly: `name` is the keyword after `@`,
 * `prelude` the text before the block (the media/supports condition, the layer
 * name, the keyframes name, ...), `rules` the nested block (empty for statement
 * at-rules like @import). The caller decides which at-rules it understands
 * (@media/@supports/@layer) and ignores the rest; lexbor already parsed the
 * nested rules regardless, so nothing is lost at this layer. */
static VALUE
mkr_css_at_rule_hash(mkr_css_conv_t *c, lxb_css_rule_at_t *at, unsigned depth)
{
    lxb_css_rule_list_t *block = mkr_css_at_block(at);
    lxb_css_rule_t *block_first = (block != NULL) ? block->first : NULL;

    VALUE h = rb_hash_new();
    rb_hash_aset(h, ID2SYM(id_type), ID2SYM(id_sym_at_rule));
    rb_hash_aset(h, ID2SYM(id_name), mkr_css_at_name(at));
    rb_hash_aset(h, ID2SYM(id_prelude),
                 mkr_css_slice_trim(c, at->prelude_begin, at->prelude_end));
    rb_hash_aset(h, ID2SYM(id_rules),
                 mkr_css_rules_to_ary(c, block_first, depth + 1));
    return h;
}

/* Convert a sibling chain of rules (first..NULL) into an Array, surfacing every
 * style / at-rule / recovered bad-style rule in source order. */
static VALUE
mkr_css_rules_to_ary(mkr_css_conv_t *c, lxb_css_rule_t *first, unsigned depth)
{
    if (depth > MKR_LEXBOR_CSS_MAX_DEPTH) {
        rb_raise(mkr_eError, "CSS at-rule nesting too deep (max %u)",
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
            case LXB_CSS_RULE_BAD_STYLE:
                /* Recovered selector lexbor rejected (e.g. a pseudo-element):
                 * surface the raw prelude for the caller to re-validate. */
                entry = mkr_css_bad_style_hash(c, lxb_css_rule_bad_style(r));
                break;
            default:
                /* Anything else error recovery dropped: do not surface it. */
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
    id_type          = rb_intern("type");
    id_selectors     = rb_intern("selectors");
    id_declarations  = rb_intern("declarations");
    id_name          = rb_intern("name");
    id_value         = rb_intern("value");
    id_important     = rb_intern("important");
    id_text          = rb_intern("text");
    id_specificity   = rb_intern("specificity");
    id_selector_text = rb_intern("selector_text");
    id_prelude       = rb_intern("prelude");
    id_rules         = rb_intern("rules");
    id_sym_style     = rb_intern("style");
    id_sym_bad_style = rb_intern("bad_style");
    id_sym_at_rule   = rb_intern("at_rule");

    VALUE mod = rb_define_module_under(mkr_mLexbor, "CSS");
    rb_define_module_function(mod, "parse_stylesheet",
                              mkr_lexbor_css_parse_stylesheet, 1);
}
