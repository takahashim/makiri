/* mkr_xpath_err.c - the engine's error and public-value helpers.
 *
 * Split out of mkr_xpath.c because of one of them: mkr_err_setf is variadic,
 * and a variadic C function cannot be DEFINED in stable Rust (only called). The
 * glue still calls it, so this file is the part of the engine that stays C
 * until the glue itself moves - a boundary drawn by the language, not by
 * design, which is why it is a file of its own rather than a leftover inside a
 * file the Rust driver replaces.
 *
 * mkr_xpath_error_clear / mkr_xpath_value_clear come along because they free
 * what these produce, and every caller that sets an error also clears one.
 */
#include "mkr_xpath.h"
#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

void
mkr_err_set(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *msg)
{
  if (err == NULL) return;
  free(err->message);
  err->status  = status;
  err->message = msg ? mkr_strdup(msg) : NULL;
}

void
mkr_err_setf(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *fmt, ...)
{
  if (err == NULL) return;
  free(err->message);
  err->status = status;
  va_list ap;
  va_start(ap, fmt);
  char buf[512];
  vsnprintf(buf, sizeof(buf), fmt, ap);
  va_end(ap);
  err->message = mkr_strdup(buf);
}

void
mkr_xpath_error_clear(mkr_xpath_error_t *e)
{
  if (e == NULL) return;
  free(e->message);
  e->message = NULL;
  e->status  = MKR_XPATH_OK;
}

void
mkr_xpath_value_clear(mkr_xpath_value_t *v)
{
  if (v == NULL) return;
  switch (v->type) {
  case MKR_XPATH_TYPE_NODESET:
    free(v->u.nodeset.nodes);
    v->u.nodeset.nodes = NULL;
    v->u.nodeset.count = 0;
    break;
  case MKR_XPATH_TYPE_STRING:
    mkr_owned_text_clear(&v->u.string);
    break;
  default:
    break;
  }
}
