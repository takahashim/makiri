/* mkr_xpath_engine_html.c - the HTML monomorphization of the XPath engine.
 *
 * One translation unit holding the whole per-instance engine: the value model,
 * the built-in function library, and the evaluator. The prelude binds the
 * MKR_NODE_* / MKR_DOM_* contract to Lexbor's lxb_dom (mkr_xpath_engine_xml.c is
 * the identical build for the custom XML node). Because all three bodies share
 * this one TU, every helper they pass between each other is file-static - the
 * engine internals never collide with the XML instance and need no per-symbol
 * renaming. Only the two node-dereferencing ENTRY points the driver dispatches
 * on (mkr_eval_ast, mkr_try_first_match) are external, suffixed _html by the
 * prelude. The representation-independent primitives both instances call live
 * once in mkr_xpath_shared.c. */
#include "mkr_xpath_prelude_html.h"

#include "mkr_xpath_value_body.h"
#include "mkr_xpath_funcs_body.h"
#include "mkr_xpath_eval_body.h"
