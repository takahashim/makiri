#ifndef MAKIRI_GLUE_RUBY_XPATH_H
#define MAKIRI_GLUE_RUBY_XPATH_H

#include "glue.h"
#include "../xpath/mkr_xpath.h"   /* mkr_xpath_value_t / mkr_xpath_error_t */

/* XPath result / error -> Ruby. Shared by the HTML query glue (ruby_xpath.c,
 * which defines them) and the XML query glue (ruby_xml.c), so the value-type
 * switch and the error->exception mapping live in exactly one place.
 *
 * mkr_xpath_value_to_ruby converts a just-produced engine value to a Ruby object
 * (NodeSet / String / Float / boolean), pushing node-set members through the
 * kind-aware NodeSet so it works for both HTML and XML documents, AND clears +v+
 * (takes ownership of its heap). mkr_xpath_raise maps +err+ to the matching
 * exception class and raises (never returns), clearing +err+ first. */
VALUE mkr_xpath_value_to_ruby(mkr_xpath_value_t *v, VALUE document);
NORETURN(void mkr_xpath_raise(mkr_xpath_error_t *err));

#endif /* MAKIRI_GLUE_RUBY_XPATH_H */
