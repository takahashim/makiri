/* mkr_xpath_number.c - XPath 1.0 Number parsing, grammar-exact and
 * locale-independent.
 *
 * The Number production is `Digits ('.' Digits?)? | '.' Digits` - no sign, no
 * exponent, no hex, decimal point only. C strtod accepts a superset of that
 * (hex floats, exponents, INF/NAN words) and honours LC_NUMERIC, so the engine
 * never hands strtod an unscanned buffer: the extent scanner below bounds the
 * exact grammar bytes first, and the converter parses only those.
 *
 * This file is the ONE home of strtod in the engine (the lexer and the
 * string->number coercion both come through here). It is compiled once and is
 * representation-independent: it never touches a DOM node, only bytes.
 */
#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <math.h>
#include <stdlib.h>

static inline int
mkr_is_ascii_digit(int c)
{
  return c >= '0' && c <= '9';
}

size_t
mkr_xpath_number_extent(const char *p, size_t len)
{
  /* All reads through the bounded span: mkr_span_peek is -1 past the end, so
   * mkr_is_ascii_digit fails closed there and no scan can leave [p, p+len). */
  mkr_span_t s = mkr_span(p, len);
  const char *start = mkr_span_mark(&s);

  if (mkr_is_ascii_digit(mkr_span_peek(&s))) {
    /* Digits ('.' Digits?)?  -> "5", "5.", "5.5" */
    while (mkr_is_ascii_digit(mkr_span_peek(&s))) mkr_span_skip(&s, 1);
    if (mkr_span_peek(&s) == '.') {
      mkr_span_skip(&s, 1);
      while (mkr_is_ascii_digit(mkr_span_peek(&s))) mkr_span_skip(&s, 1);
    }
    return mkr_span_since(&s, start);
  }
  if (mkr_span_peek(&s) == '.') {
    /* '.' Digits  -> ".5"  (a bare "." is NOT a Number) */
    mkr_span_skip(&s, 1);
    if (!mkr_is_ascii_digit(mkr_span_peek(&s))) return 0;
    while (mkr_is_ascii_digit(mkr_span_peek(&s))) mkr_span_skip(&s, 1);
    return mkr_span_since(&s, start);
  }
  return 0;
}

/* Allocation-free, locale-independent assembly of the (already grammar-checked)
 * extent. Reached only when LC_NUMERIC != C makes the libc '.' parse fail, or
 * when the isolating reparse copy can't be allocated (OOM). This corner trades
 * correctly-rounded parsing for locale independence and never-failing: it builds
 * the value digit-by-digit (libxml2-precision-class), so it can never raise and
 * never mis-classifies the grammar. */
static double
mkr_xpath_number_manual(const char *p, size_t extent)
{
  double v = 0.0;
  size_t i = 0;
  for (; i < extent && p[i] >= '0' && p[i] <= '9'; i++) {
    v = v * 10.0 + (double)(p[i] - '0');
  }
  if (i < extent && p[i] == '.') {
    i++;
    double scale = 1.0;
    for (; i < extent && p[i] >= '0' && p[i] <= '9'; i++) {
      scale /= 10.0;
      v += (double)(p[i] - '0') * scale;
    }
  }
  return v;
}

double
mkr_xpath_number_from_extent(const char *p, size_t extent)
{
  if (p == NULL || extent == 0) return (double)NAN;

  /* Fast path (the hot, common case): the engine text contract NUL-terminates
   * the buffer at/after p+extent, so this strtod is bounded and cannot leave the
   * buffer; in the C locale it parses exactly the grammar bytes - the extent
   * holds no sign/exponent/hex/INF/NAN for strtod to over-consume. When it
   * consumed precisely `extent` bytes we are done, with no allocation. */
  char *end = NULL;
  double v = strtod(p, &end);
  if (end != NULL && (size_t)(end - p) == extent) return v;

  /* Disagreement. Either strtod consumed MORE (a number-like continuation abuts
   * the extent in the surrounding buffer, e.g. "1e3" / "0x10" where only "1" /
   * "0" is the Number) - excluded by the grammar - or LESS (a comma-decimal
   * LC_NUMERIC stopped at '.'). Reparse exactly the extent in isolation. The
   * copy is made ONLY here, off the hot path. */
  char *copy = mkr_strndup(p, extent);
  if (copy != NULL) {
    char *cend = NULL;
    double cv = strtod(copy, &cend);
    int full = (cend != NULL && (size_t)(cend - copy) == extent);
    free(copy);
    if (full) return cv;
    /* parsed but stopped short -> LC_NUMERIC '.' failure: fall through. */
  }
  /* OOM on the copy, or the locale '.' failure: assemble by hand (no alloc), so
   * we fail closed rather than turning an OOM into a NaN or a raise. */
  return mkr_xpath_number_manual(p, extent);
}
