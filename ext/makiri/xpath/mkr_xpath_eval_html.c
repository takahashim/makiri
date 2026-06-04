/* mkr_xpath_eval_html.c — HTML instance of the XPath evaluator.
 *
 * The evaluator logic is the shared, representation-neutral body in
 * mkr_xpath_eval_body.h. This wrapper binds the MKR_NODE_* node-access contract
 * to lxb_dom (mkr_xpath_node_access_html.h) and compiles the body for HTML. The XML
 * instance (mkr_xpath_eval_xml.c) will include the same body with the macros
 * bound to the custom mkr_xml_node_t. Keep node access going through the macros
 * so both instances stay byte-identical to a direct field read (§2.5). */
#include "mkr_xpath_html_prelude.h"
#include "mkr_xpath_eval_body.h"
