/* mkr_xpath_html_prelude.h — HTML engine-instance prelude.
 *
 * Included at the very top of each HTML engine wrapper (eval_html.c /
 * funcs_html.c / value_html.c) BEFORE the shared body, symmetric with
 * mkr_xpath_xml_prelude.h. It binds the DOM types to Lexbor and pulls in the
 * HTML node-access contract, so the shared body compiles against lxb_dom. (The
 * per-instance symbol renames live below once the representation-dependent
 * functions are split from the shared helpers.)
 */
#ifndef MKR_XPATH_HTML_PRELUDE_H
#define MKR_XPATH_HTML_PRELUDE_H

#include <lexbor/dom/dom.h>

/* DOM types -> Lexbor (the type counterpart of MKR_NODE_*). */
#define MKR_DOM_NODE     lxb_dom_node_t
#define MKR_DOM_ELEMENT  lxb_dom_element_t
#define MKR_DOM_ATTR     lxb_dom_attr_t
#define MKR_DOM_DOCUMENT lxb_dom_document_t

#include "mkr_xpath_node_access_html.h"   /* MKR_NODE_* for lxb_dom */

#endif /* MKR_XPATH_HTML_PRELUDE_H */
