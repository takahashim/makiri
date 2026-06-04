/* mkr_xpath_funcs_xml.c — XML instance of the XPath funcs body.
 *
 * The prelude binds the node-access contract + DOM types to the custom node and
 * suffixes the body's symbols with _xml; then the shared body compiles for XML.
 * See mkr_xpath_funcs.c for the HTML instance and §2.5 for the rationale. */
#include "mkr_xpath_xml_prelude.h"
#include "mkr_xpath_funcs_body.h"
