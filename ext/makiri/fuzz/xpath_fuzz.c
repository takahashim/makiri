/* xpath_fuzz.c - libFuzzer harness for the XPath compile + eval path.
 *
 * Coverage-guided fuzzing of the XPath lexer, parser, and evaluator.
 * We build a small fixed XML document as the evaluation context, then
 * treat the fuzzer input as the XPath expression string.
 *
 * Ruby-free; runs directly on the pure-C engine surface.
 */

#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "core/mkr_alloc.h"
#include "xml/mkr_xml.h"
#include "xpath/mkr_xpath.h"
#include "xpath/mkr_xpath_internal.h"

/* A small, fixed XML document that gives the evaluator something to walk.
 * The expression is the fuzzer input; the document is static so the coverage
 * signal comes from the engine, not the parser. */
static const char FIXED_XML[] =
    "<?xml version='1.0'?>"
    "<root xmlns='http://example.com/default' xmlns:ns='http://example.com/ns'>"
    "  <a id='1' ns:attr='x'>text1</a>"
    "  <b id='2'><c/><c/></b>"
    "  <ns:d>namespaced</ns:d>"
    "  <!-- comment -->"
    "  <?pi target='value'?>"
    "</root>";

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size)
{
    /* 1. Parse the fixed document. */
    mkr_xml_status_t status;
    mkr_xml_doc_t *doc = mkr_xml_parse(FIXED_XML, sizeof(FIXED_XML) - 1, &status);
    if (!doc) return 0;
    if (!doc->doc_node) {
        mkr_xml_doc_destroy(doc);
        return 0;
    }

    /* 2. The fuzzer input is the XPath expression.
     *    The engine text contract requires no interior NUL and a NUL at
     *    ptr[len]. libFuzzer hands us exactly `size` bytes with no terminator,
     *    so we copy the expression prefix into an owned, NUL-terminated heap
     *    buffer and mint the verified-text token over that copy - this is what
     *    supplies the NUL-termination + no-interior-NUL the lexer's strtod and
     *    "%.10s" error path rely on. If the input contains a NUL we truncate to
     *    the prefix (the lexer hits the terminator and reports a syntax error,
     *    a path worth exercising). UTF-8 validity is deliberately NOT
     *    pre-checked: the lexer's strict decoder rejecting invalid UTF-8 is
     *    itself a path the fuzzer should hit. */
    size_t expr_len = size;
    for (size_t i = 0; i < size; i++) {
        if (data[i] == '\0') {
            expr_len = i;
            break;
        }
    }
    char *expr_copy = mkr_strndup((const char *) data, expr_len);
    if (!expr_copy) {
        mkr_xml_doc_destroy(doc);
        return 0;
    }
    /* Empty expression is a quick syntax error; still worth a run. */
    mkr_verified_text_t expr = { expr_copy, expr_len };

    /* 3. Compile the expression. */
    mkr_xpath_limits_t limits;
    mkr_xpath_limits_init_defaults(&limits);
    /* Tighten the compile-time budgets so a hostile expression fails fast
     * rather than burning fuzzer time on pathological ASTs. */
    limits.max_ast_nodes = 10000;
    limits.max_expr_bytes = 16 * 1024;

    mkr_xpath_error_t err = {0};
    mkr_node_t *ast = mkr_parse(expr, &limits, &err);
    if (!ast) {
        mkr_xpath_error_clear(&err);
        free(expr_copy);
        mkr_xml_doc_destroy(doc);
        return 0;
    }

    /* 4. Evaluate against the fixed document. */
    mkr_xpath_context_t *ctx = mkr_xpath_context_new(doc->doc_node, doc->doc_node);
    if (ctx) {
        mkr_xpath_set_engine_kind(ctx, 1); /* XML engine */
        mkr_xpath_limits_init_defaults(&limits);
        limits.max_eval_ops        = 5 * 1000 * 1000; /* 5M ops - enough for a real query */
        limits.max_nodeset_size    = 10000;
        limits.max_string_bytes    = 1024 * 1024;
        limits.max_recursion_depth = 64;

        mkr_xpath_value_t out = {0};
        mkr_xpath_error_t eval_err = {0};
        if (mkr_xpath_eval_compiled(ctx, ast, &out, &eval_err) == 0) {
            mkr_xpath_value_clear(&out);
        } else {
            mkr_xpath_error_clear(&eval_err);
        }
        mkr_xpath_context_free(ctx);
    }

    mkr_node_free(ast);
    free(expr_copy);
    mkr_xml_doc_destroy(doc);
    return 0;
}
