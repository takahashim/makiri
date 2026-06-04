/* mkr_xpath_funcs.c — HTML instance of the XPath built-in functions.
 *
 * Shared body: mkr_xpath_funcs_body.h. This wrapper binds the node-access
 * contract to lxb_dom and compiles the function library for HTML. See
 * mkr_xpath_eval.c for the monomorphization rationale (§2.5). */
#include "mkr_node_access.h"
#include "mkr_xpath_funcs_body.h"
