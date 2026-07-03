/* CBMC harness (experimental, cbmc-deep): the XPath lexer alone, over a
 * nondet expression of up to VERIFY_XPATH_EXPR_MAX bytes.
 *
 * The byte-scanning layer is where out-of-bounds reads are born, so this is
 * the natural first step below the full parser entry - but as of writing even
 * this instance does not converge in practical time on current bit-level
 * solvers (>6 min at 6 bytes; excluding the number path did not help). Kept
 * for future solver/harness work; see the Makefile's cbmc-deep note.
 *
 * The input honours the engine contract (valid UTF-8, no NUL) via
 * assumptions; within it, every byte sequence is covered. Properties: no
 * OOB/UB while tokenizing (--bounds-check etc.), and the token stream
 * terminates cleanly (EOF or error) within the input's byte count.
 */
#include "verify.h"
#include "core/mkr_core.h"
#include "xpath/mkr_xpath.h"
#include "xpath/mkr_xpath_internal.h"

#include <stdbool.h>

#ifndef VERIFY_XPATH_EXPR_MAX
#define VERIFY_XPATH_EXPR_MAX 6
#endif

int
main(void)
{
    char buf[VERIFY_XPATH_EXPR_MAX];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    bool utf8_ok = mkr_utf8_valid((const unsigned char *)buf, len);
    VERIFY_ASSUME(utf8_ok);
    for (size_t i = 0; i < len; ++i) VERIFY_ASSUME(buf[i] != '\0');

    mkr_xpath_error_t err = {0};
    mkr_lexer_t L;
    mkr_lexer_init(&L, buf, len, &err);

    /* Each token consumes at least one byte, so EOF/error arrives within
     * len + 1 advances; the fixed loop bound keeps the unwinding exact. */
    for (int i = 0; i <= VERIFY_XPATH_EXPR_MAX && L.good; ++i) {
        (void)mkr_lexer_peek_nonws(&L);
        if (mkr_lexer_advance(&L, &err) != 0) break;
        if (L.tok.kind == MKR_TK_EOF) break;
    }
    mkr_xpath_error_clear(&err);
    return 0;
}
