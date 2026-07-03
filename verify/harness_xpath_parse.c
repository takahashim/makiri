/* CBMC harness: the XPath lexer + recursive-descent parser over a nondet
 * expression of up to VERIFY_XPATH_EXPR_MAX bytes.
 *
 * The input honours the engine's contract (mkr_verified_text_t: valid UTF-8,
 * no NUL) via assumptions; within it, every byte sequence is covered.
 * Properties: no OOB/UB anywhere in lex/parse (--bounds-check etc.), and the
 * entry either returns an AST or fails closed with the error filled. The
 * parser's recursion is bounded by max_recursion_depth, which the harness sets
 * low so --unwind stays small.
 */
#include "verify.h"
#include "core/mkr_core.h"
#include "xpath/mkr_xpath.h"

#include <stdbool.h>

#ifndef VERIFY_XPATH_EXPR_MAX
#define VERIFY_XPATH_EXPR_MAX 6
#endif

/* Defined in the engine (declared in mkr_xpath_internal.h, which drags the
 * monomorphization machinery in; declare just this entry instead). */
void mkr_xpath_limits_init_defaults(mkr_xpath_limits_t *L);

int
main(void)
{
    char buf[VERIFY_XPATH_EXPR_MAX];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    /* mkr_verified_text_t contract: valid UTF-8, no embedded NUL. */
    bool utf8_ok = mkr_utf8_valid((const unsigned char *)buf, len);
    VERIFY_ASSUME(utf8_ok);
    for (size_t i = 0; i < len; ++i) VERIFY_ASSUME(buf[i] != '\0');

    mkr_xpath_limits_t L;
    mkr_xpath_limits_init_defaults(&L);
    L.max_expr_bytes      = VERIFY_XPATH_EXPR_MAX;
    L.max_ast_nodes       = 16;
    L.max_steps           = 4;
    L.max_predicates      = 2;
    L.max_function_args   = 2;
    L.max_recursion_depth = 6;

    mkr_xpath_error_t err = {0};
    mkr_verified_text_t expr = { buf, len };
    mkr_node_t *ast = mkr_parse(expr, &L, &err);
    if (ast != NULL) {
        mkr_node_free(ast);
    } else {
        VERIFY_ASSERT(err.status != 0, "parse failure fills the error");
    }
    mkr_xpath_error_clear(&err);
    return 0;
}
