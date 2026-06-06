/* mkr_xpath_engine_xml.c - the XML monomorphization of the XPath engine.
 *
 * Byte-for-byte the same bodies as mkr_xpath_engine_html.c, but the prelude
 * binds the MKR_NODE_* / MKR_DOM_* contract to the custom mkr_xml_node_t and
 * selects the engine's XML host-policy branches (MKR_HOST_XML). See
 * mkr_xpath_engine_html.c for the one-TU / file-static rationale; here the two
 * external entries are suffixed _xml. */
#include "mkr_xpath_prelude_xml.h"

#include "mkr_xpath_value_body.h"
#include "mkr_xpath_funcs_body.h"
#include "mkr_xpath_eval_body.h"
