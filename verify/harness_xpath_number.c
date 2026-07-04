/* CBMC harness: the XPath Number scanner + converter (xpath/mkr_xpath_number.c)
 * over every byte sequence up to VERIFY_XPATH_NUM_MAX bytes.
 *
 * The scanner (mkr_xpath_number_extent) is the lexer's and the string->number
 * coercion's shared front - the one place that decides how many bytes belong
 * to a Number. Properties:
 *   - the extent never passes len (no OOB - and --bounds-check watches the
 *     scan itself);
 *   - a non-zero extent is shaped like the XPath 1.0 Number grammar:
 *     digits and at most one '.', starting with a digit or a '.'-then-digit,
 *     never ending in a lone '.' unless digits preceded it (Digits '.'? |
 *     '.' Digits);
 *   - the converter on a scanner-approved extent yields a non-negative,
 *     non-NaN double (the grammar has no sign, no exponent, no NaN spelling).
 */
#include "verify.h"

#include <math.h>
#include <stdbool.h>
#include <stddef.h>

/* Declared in mkr_xpath_internal.h, which drags the engine monomorphization
 * machinery in; declare the two entries directly instead. */
size_t mkr_xpath_number_extent(const char *p, size_t len);
double mkr_xpath_number_from_extent(const char *p, size_t extent);

#ifndef VERIFY_XPATH_NUM_MAX
#define VERIFY_XPATH_NUM_MAX 8
#endif

static bool
is_digit(char c)
{
    return c >= '0' && c <= '9';
}

int
main(void)
{
    /* +1: the engine text contract NUL-terminates the expression buffer at
     * [len] (from_extent's fast path relies on it to bound strtod). */
    char buf[VERIFY_XPATH_NUM_MAX + 1];
    for (size_t i = 0; i + 1 < sizeof buf; ++i) buf[i] = (char)(unsigned char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= VERIFY_XPATH_NUM_MAX);
    buf[len] = '\0';

    size_t ext = mkr_xpath_number_extent(buf, len);
    VERIFY_ASSERT(ext <= len, "extent: never past len");

    if (ext > 0) {
        size_t dots = 0, digits = 0;
        for (size_t i = 0; i < ext; ++i) {
            VERIFY_ASSERT(is_digit(buf[i]) || buf[i] == '.',
                          "extent: only digits and '.'");
            if (buf[i] == '.') dots++; else digits++;
        }
        VERIFY_ASSERT(dots <= 1, "extent: at most one '.'");
        VERIFY_ASSERT(digits >= 1, "extent: at least one digit");
        VERIFY_ASSERT(is_digit(buf[0]) || (buf[0] == '.' && ext >= 2 && is_digit(buf[1])),
                      "extent: starts per the Number grammar");

        double v = mkr_xpath_number_from_extent(buf, ext);
        VERIFY_ASSERT(!isnan(v), "convert: a scanned Number is never NaN");
        VERIFY_ASSERT(v >= 0.0, "convert: the grammar has no sign");
    }
    return 0;
}
