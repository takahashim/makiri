/* mkr_xpath_prelude_xml.h - XML engine-instance prelude.
 *
 * Included at the top of mkr_xpath_engine_xml.c BEFORE the engine bodies. It
 * binds the DOM types + MKR_NODE_* node-access contract to the custom
 * mkr_xml_node_t (and selects the engine's XML host-policy via MKR_HOST_XML in
 * the node-access header), so the same bodies compile for XML.
 *
 * The engine's internals are file-static (one merged TU per instance), so they
 * coexist with the HTML instance without renaming. Only the two
 * node-dereferencing ENTRY points the driver dispatches on are external; the
 * prelude suffixes them _xml (the HTML prelude suffixes the same pair _html).
 */
#ifndef MKR_XPATH_PRELUDE_XML_H
#define MKR_XPATH_PRELUDE_XML_H

/* DOM types -> the custom node (the type counterpart of MKR_NODE_*). */
#define MKR_DOM_NODE     mkr_xml_node_t
#define MKR_DOM_ELEMENT  mkr_xml_node_t
#define MKR_DOM_ATTR     mkr_xml_node_t
#define MKR_DOM_DOCUMENT mkr_xml_doc_t

/* The two external entry points (the only symbols not file-static). */
#define mkr_eval_ast        mkr_eval_ast_xml
#define mkr_try_first_match mkr_try_first_match_xml

#include "mkr_xpath_node_access_xml.h"   /* MKR_NODE_* + MKR_HOST_XML for the custom node */

#endif /* MKR_XPATH_PRELUDE_XML_H */
