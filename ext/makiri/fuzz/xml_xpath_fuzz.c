/* xml_xpath_fuzz.c - libFuzzer harness where the fuzzer controls BOTH the
 * document and the expression.
 *
 * Complements the other two targets: xml_fuzz covers the parser alone, and
 * xpath_fuzz covers the engine over one fixed document shape - so evaluator
 * paths that depend on document structure (namespace scopes, index buckets,
 * axis edge cases) only ever see that shape. Here the input's bytes up to the
 * first NUL are the XML document and the rest is the expression (both engine
 * contracts forbid embedded NUL, so the separator costs no coverage).
 *
 * Contract violations (invalid UTF-8, non-XML chars) are filtered the same
 * way the Ruby bridge does, so the harness only exercises in-contract inputs;
 * the validators themselves run on every input and are fuzzed for free.
 * Ruby-free; the XML arena self-poisons under ASan, so intra-arena overruns
 * are caught. */
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "core/mkr_core.h"
#include "xml/mkr_xml.h"
#include "xpath/mkr_xpath.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

int
LLVMFuzzerTestOneInput(const uint8_t *data, size_t size)
{
    const uint8_t *sep = memchr(data, 0, size);
    if (sep == NULL) return 0;

    const char *xml  = (const char *)data;
    size_t xml_len   = (size_t)(sep - data);
    const char *expr = (const char *)sep + 1;
    size_t expr_len  = size - xml_len - 1;

    /* In-contract filter, mirroring the bridge's strict gate. */
    if (!mkr_utf8_valid((const unsigned char *)xml, xml_len)) return 0;
    if (mkr_xml_validate_chars(xml, (uint32_t)xml_len) != 0) return 0;
    if (!mkr_utf8_valid((const unsigned char *)expr, expr_len)) return 0;

    mkr_xml_status_t st = MKR_XML_OK;
    mkr_xml_doc_t *doc = mkr_xml_parse(xml, xml_len, &st);
    if (doc == NULL) return 0;

    mkr_xpath_context_t *ctx = mkr_xpath_context_new(doc->doc_node, doc->doc_node);
    if (ctx != NULL) {
        mkr_xpath_set_engine_kind(ctx, 1); /* XML monomorphization */

        /* Shrink the per-evaluate budgets so a single hostile input can't
         * stall the fuzzer; overruns fail closed, which is the point. */
        mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
        L->max_eval_ops     = 20000;
        L->max_nodeset_size = 1024;
        L->max_string_bytes = 4096;

        mkr_verified_text_t dp = { "d", 1 }, du = { "urn:d", 5 };
        mkr_xpath_register_ns(ctx, dp, du);

        mkr_xpath_error_t err = {0};
        mkr_verified_text_t e = { expr, expr_len };
        mkr_node_t *ast = mkr_parse(e, L, &err);
        if (ast != NULL) {
            mkr_xpath_value_t v = {0};
            (void)mkr_xpath_eval_compiled(ctx, ast, &v, &err);
            mkr_xpath_value_clear(&v);
            mkr_node_free(ast);
        }
        mkr_xpath_error_clear(&err);
        mkr_xpath_context_free(ctx);
    }
    mkr_xml_doc_destroy(doc);
    return 0;
}
