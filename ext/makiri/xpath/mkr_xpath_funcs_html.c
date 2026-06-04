/* mkr_xpath_funcs_html.c — HTML instance of the XPath built-in functions.
 *
 * Shared body: mkr_xpath_funcs_body.h. This wrapper binds the node-access
 * contract to lxb_dom and compiles the function library for HTML. See
 * mkr_xpath_eval_html.c for the monomorphization rationale (§2.5). */
#include "mkr_xpath_node_access_html.h"
#include "mkr_xpath_funcs_body.h"
