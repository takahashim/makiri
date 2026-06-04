/* mkr_xpath_html_prelude.h — HTML engine-instance prelude.
 *
 * Included at the top of mkr_xpath_engine_html.c BEFORE the engine bodies,
 * symmetric with mkr_xpath_xml_prelude.h. It binds the DOM types + the
 * MKR_NODE_* node-access contract to Lexbor's lxb_dom, so the bodies compile
 * against lxb_dom.
 *
 * The engine's internals are file-static (one merged TU per instance), so they
 * never collide with the XML instance and need no renaming. Only the two
 * node-dereferencing ENTRY points the driver dispatches on are external; the
 * prelude suffixes them _html so they coexist with the XML instance's _xml
 * pair. (mkr_xpath.c declares both and selects by engine_kind.) */
#ifndef MKR_XPATH_HTML_PRELUDE_H
#define MKR_XPATH_HTML_PRELUDE_H

#include <lexbor/dom/dom.h>

/* DOM types -> Lexbor (the type counterpart of MKR_NODE_*). */
#define MKR_DOM_NODE     lxb_dom_node_t
#define MKR_DOM_ELEMENT  lxb_dom_element_t
#define MKR_DOM_ATTR     lxb_dom_attr_t
#define MKR_DOM_DOCUMENT lxb_dom_document_t

/* The two external entry points (the only symbols not file-static). */
#define mkr_eval_ast        mkr_eval_ast_html
#define mkr_try_first_match mkr_try_first_match_html

#include "mkr_xpath_node_access_html.h"   /* MKR_NODE_* for lxb_dom */

#endif /* MKR_XPATH_HTML_PRELUDE_H */
