/* mkr_xpath_xml_prelude.h — XML engine-instance prelude.
 *
 * Included at the very top of each XML engine wrapper (eval_xml.c / funcs_xml.c
 * / value_xml.c) BEFORE the shared body. It binds the DOM types to the custom
 * node and suffixes every externally-linked body symbol with _xml, so the XML
 * instance's object code coexists with the HTML instance (same body, two
 * monomorphizations). The pointer-only / string-only helpers exist in both
 * instances as identical copies; mkr_xpath.c calls the HTML copy and dispatches
 * only the two node-dereferencing entries (mkr_eval_ast, mkr_try_first_match).
 */
#ifndef MKR_XPATH_XML_PRELUDE_H
#define MKR_XPATH_XML_PRELUDE_H

/* DOM types -> the custom node (the type counterpart of MKR_NODE_*). */
#define MKR_DOM_NODE     mkr_xml_node_t
#define MKR_DOM_ELEMENT  mkr_xml_node_t
#define MKR_DOM_ATTR     mkr_xml_node_t
#define MKR_DOM_DOCUMENT mkr_xml_doc_t

/* Per-instance symbol renames (every non-static body function). */
#define mkr_apply_peephole                 mkr_apply_peephole_xml
#define mkr_borrowed_text_eq               mkr_borrowed_text_eq_xml
#define mkr_borrowed_text_to_number        mkr_borrowed_text_to_number_xml
#define mkr_doc_order_index_clear          mkr_doc_order_index_clear_xml
#define mkr_doc_order_index_init           mkr_doc_order_index_init_xml
#define mkr_eval_ast                       mkr_eval_ast_xml
#define mkr_get_cached_node_text           mkr_get_cached_node_text_xml
#define mkr_lookup_function                mkr_lookup_function_xml
#define mkr_mark_context_independent       mkr_mark_context_independent_xml
#define mkr_node_clear_memos               mkr_node_clear_memos_xml
#define mkr_node_free                      mkr_node_free_xml
#define mkr_node_to_owned_text_or_fail     mkr_node_to_owned_text_or_fail_xml
#define mkr_nodeset_clear                  mkr_nodeset_clear_xml
#define mkr_nodeset_init                   mkr_nodeset_init_xml
#define mkr_nodeset_push                   mkr_nodeset_push_xml
#define mkr_nodeset_sort_doc_order         mkr_nodeset_sort_doc_order_xml
#define mkr_nodeset_unique_sorted          mkr_nodeset_unique_sorted_xml
#define mkr_owned_text_clear               mkr_owned_text_clear_xml
#define mkr_owned_text_from_borrowed_copy  mkr_owned_text_from_borrowed_copy_xml
#define mkr_owned_text_from_buf_steal      mkr_owned_text_from_buf_steal_xml
#define mkr_owned_text_init                mkr_owned_text_init_xml
#define mkr_step_clear                     mkr_step_clear_xml
#define mkr_str_cache_clear                mkr_str_cache_clear_xml
#define mkr_str_cache_init                 mkr_str_cache_init_xml
#define mkr_str_cache_truncate             mkr_str_cache_truncate_xml
#define mkr_try_first_match                mkr_try_first_match_xml
#define mkr_val_clear                      mkr_val_clear_xml
#define mkr_val_clone                      mkr_val_clone_xml
#define mkr_val_set_borrowed_text_copy     mkr_val_set_borrowed_text_copy_xml
#define mkr_val_set_owned_text             mkr_val_set_owned_text_xml
#define mkr_val_to_boolean                 mkr_val_to_boolean_xml
#define mkr_val_to_number_or_fail          mkr_val_to_number_or_fail_xml
#define mkr_val_to_number_unchecked        mkr_val_to_number_unchecked_xml
#define mkr_val_to_owned_text_or_fail      mkr_val_to_owned_text_or_fail_xml

#include "mkr_node_access_xml.h"   /* MKR_NODE_* + MKR_HOST_XML for the custom node */

#endif /* MKR_XPATH_XML_PRELUDE_H */
