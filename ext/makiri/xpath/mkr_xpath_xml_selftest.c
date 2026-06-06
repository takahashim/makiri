/* mkr_xpath_xml_selftest.c - exercise the XML engine instance end to end.
 *
 * Compiled as a normal TU (HTML default MKR_DOM_* types), it parses a small XML
 * document and drives the shared engine entry (mkr_xpath_eval_compiled), which
 * dispatches to the _xml monomorphization because the context's engine kind is
 * XML. This proves the XML instance evaluates correctly before the Ruby Node API
 * (step 2b) wires it up. Run from Makiri.__c_selftest under ASan/UBSan. */
#include "mkr_xpath.h"
#include "mkr_xpath_internal.h"
#include "../xml/mkr_xml.h"

/* Evaluate +expr+ (length +elen+) against the XML +doc+ and return the resulting
 * node-set count, or SIZE_MAX on any error / non-node-set. Registers prefix
 * "d" -> "urn:d". Lengths come from sizeof on the compile-time literals (no
 * strlen - this file is outside the clint core+bridge allowlist). */
static size_t
xml_count(mkr_xml_doc_t *doc, const char *expr, size_t elen)
{
    /* The context's "document" and root context node are both the DOCUMENT node
     * (the XPath "/" root); for XML the engine's namespace services ignore the
     * document handle, so storing the node there is safe. */
    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((MKR_DOM_DOCUMENT *)doc->doc_node, (MKR_DOM_NODE *)doc->doc_node);
    if (ctx == NULL) return (size_t)-1;
    mkr_xpath_set_engine_kind(ctx, 1);

    mkr_verified_text_t dp = { "d", 1 }, du = { "urn:d", 5 };
    mkr_xpath_register_ns(ctx, dp, du);

    mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
    mkr_xpath_error_t err = {0};
    mkr_verified_text_t e = { expr, elen };
    mkr_node_t *ast = mkr_parse(e, L, &err);
    size_t out = (size_t)-1;
    if (ast != NULL) {
        mkr_xpath_value_t v = {0};
        if (mkr_xpath_eval_compiled(ctx, ast, &v, &err) == 0
            && v.type == MKR_XPATH_TYPE_NODESET) {
            out = v.u.nodeset.count;
        }
        mkr_xpath_value_clear(&v);
        mkr_node_free(ast);
    }
    mkr_xpath_error_clear(&err);
    mkr_xpath_context_free(ctx);
    return out;
}

int
mkr_xml_xpath_selftest(void)
{
    int idx = 0;
    mkr_xml_status_t st = MKR_XML_OK;
    /* Two no-namespace <b>, one namespaced <d:b>, one <c> under the 2nd <b>. */
    static const char src[] = "<a><b/><b><c/></b><d:b xmlns:d='urn:d'/></a>";
    mkr_xml_doc_t *doc = mkr_xml_parse(src, sizeof(src) - 1, &st);
    if (doc == NULL) return ++idx;                                  /* 1 */

#define CHECK(expr, want)                                                     \
    do { idx++; if (xml_count(doc, (expr), sizeof(expr) - 1) != (size_t)(want)) { \
             mkr_xml_doc_destroy(doc); return idx; } } while (0)

    CHECK("//b", 2);          /* 1: unprefixed -> no-namespace only (excludes d:b) */
    CHECK("/a/b", 2);         /* 2 */
    CHECK("//c", 1);          /* 3 */
    CHECK("//*", 5);          /* 4: a, b, b, c, d:b */
    CHECK("//d:b", 1);        /* 5: prefixed -> namespace urn:d */
    CHECK("//b[c]", 1);       /* 6: predicate over a child */
    CHECK("//a/*", 3);        /* 7: child axis, the 3 element children of <a> */
    CHECK("//a/b[1]", 1);     /* 8: positional predicate (the first child <b>) */
    CHECK("//svg", 0);        /* 9: no such element */

#undef CHECK
    mkr_xml_doc_destroy(doc);
    return 0;
}
