#ifndef MAKIRI_GLUE_H
#define MAKIRI_GLUE_H

#include "../makiri.h"
#include "../dom_adapter/compat.h"

#ifdef __cplusplus
extern "C" {
#endif

/* A DOM node pointer of UNKNOWN representation - an HTML lxb_dom_node_t or an XML
 * mkr_xml_node_t - as stored in a node wrapper or a NodeSet. It is an INCOMPLETE
 * type on purpose: it cannot be dereferenced, and (unlike void*) it does not
 * implicitly convert to a typed pointer, so reading a stored node AS a specific
 * representation requires an explicit cast that the kind-checked accessors
 * (mkr_html_node_unwrap / mkr_xml_node_unwrap) justify by the wrapper's TypedData type
 * (or, for a NodeSet, by doc_is_xml). The stored pointer is only ever
 * pointer-compared or cast through one of those accessors. */
typedef struct mkr_raw_node mkr_raw_node_t;

/* Wrapper for any DOM node except Document. The node memory is owned by the
 * document's arena (an HTML Lexbor arena or the XML node arena); we keep only the
 * pointer plus a keepalive VALUE reference to the Ruby Document so the arena
 * outlives the wrapper. The pointer is representation-opaque (mkr_raw_node_t):
 * read it only through mkr_html_node_unwrap / mkr_xml_node_unwrap, which check the
 * wrapper's representation (distinct TypedData types) before casting. */
typedef struct {
    mkr_raw_node_t *node;
    VALUE           document;
} mkr_node_data_t;

/* Wrapper for a Document: owns the parsed result (and thus the arena). */
typedef struct {
    mkr_parsed_t *parsed;
    VALUE         errors;   /* Ruby Array; reserved for parse warnings */
} mkr_doc_data_t;

/* Hard cap on the number of nodes a single NodeSet may hold; mirrors the XPath
 * engine's max_nodeset_size so every node-collecting path (tree walks, XPath,
 * CSS) fails closed at the same bound. */
#define MKR_NODE_SET_MAX (10u * 1000u * 1000u)

extern const rb_data_type_t mkr_node_type;
extern const rb_data_type_t mkr_doc_type;
extern const rb_data_type_t mkr_node_set_type;

/* Node bridge (glue/ruby_node.c). mkr_wrap_html_node returns the Document VALUE
 * for the document node, Qnil for NULL, otherwise a freshly-wrapped Node. */
VALUE           mkr_wrap_html_node(lxb_dom_node_t *node, VALUE document);
VALUE           mkr_node_document(VALUE rb_node);

/* HTML and XML nodes are wrapped under DISTINCT TypedData types (both deriving
 * from the shared base mkr_node_type), so a representation-specific accessor
 * rejects the wrong kind via Ruby's type machinery. See ruby_node.c.
 *   mkr_html_node_unwrap      -> lxb_dom_node_t* ; raises on an XML node/Document.
 *   mkr_xml_node_unwrap-> mkr_xml_node_t* ; raises on an HTML node/Document (ruby_xml_node.c).
 *   mkr_node_raw       -> void* ; kind-agnostic raw pointer for identity, or for a
 *                         site where the kind is already guaranteed. Deref needs an
 *                         explicit cast - never treat it as a typed pointer blindly.
 *   mkr_node_id        -> uintptr_t ; node identity for ==/eql?/hash/pointer_id. */
extern const rb_data_type_t mkr_html_node_type;
extern const rb_data_type_t mkr_xml_node_type;
lxb_dom_node_t *mkr_html_node_unwrap(VALUE rb_node);
void           *mkr_node_raw(VALUE rb_node);
uintptr_t       mkr_node_id(VALUE rb_node);

/* Representation-neutral identity methods (ruby_node.c): depend only on
 * mkr_node_id, so the HTML and XML NodeMethods modules bind ==/eql? to
 * mkr_node_equals, hash to mkr_node_hash, and pointer_id to mkr_node_pointer_id -
 * one implementation, not one per representation. */
VALUE mkr_node_equals(VALUE self, VALUE other);
VALUE mkr_node_pointer_id(VALUE self);
VALUE mkr_node_hash(VALUE self);

/* XML node bridge (glue/ruby_xml_node.c): wrap a custom XML node into the right
 * Makiri::XML::* leaf (Qnil for NULL, the Document VALUE for the document node). */
struct mkr_xml_node;
VALUE mkr_wrap_xml_node(struct mkr_xml_node *node, VALUE document);
/* XML node-pointer accessor; raises TypeError on an HTML node/Document. */
struct mkr_xml_node *mkr_xml_node_unwrap(VALUE rb_node);

/* Document bridge (glue/ruby_doc.c). */
lxb_dom_document_t *mkr_html_doc_unwrap(VALUE rb_doc);
mkr_parsed_t       *mkr_doc_parsed(VALUE rb_doc);
VALUE               mkr_wrap_document(mkr_parsed_t *parsed); /* GC takes ownership */

/* Shared fragment-parse machinery (glue/ruby_doc.c), reused by ruby_mutate.c's
 * inner_html=/outer_html= so the UTF-8 sanitisation and import+template-fixup
 * are not duplicated.
 *
 * mkr_sanitize_html_input: decode rb_html for the fragment parser - *out / *out_len
 * are the bytes to parse, *owned a malloc'd buffer to free afterwards (NULL when
 * the input is used in place). Returns 0, or -1 on OOM (nothing allocated), so
 * the caller can release its parser before raising. See mkr_utf8_sanitize.
 *
 * mkr_import_fragment_children: deep-import each child of `root` into `doc`, hand
 * it to `emit`, and fix up any <template> contents (which import_node omits).
 *
 * mkr_emit_append / mkr_emit_before: emit callbacks - append as last child of
 * `u`, or insert before the reference node `u`. */
int  mkr_sanitize_html_input(VALUE html, const lxb_char_t **out, size_t *out_len,
                             lxb_char_t **owned);
void mkr_import_fragment_children(lxb_dom_document_t *doc, lxb_dom_node_t *root,
                                  void (*emit)(lxb_dom_node_t *, void *), void *u);
void mkr_emit_append(lxb_dom_node_t *imported, void *u);
void mkr_emit_before(lxb_dom_node_t *imported, void *u);

/* Shared transient-fragment-parser scaffold (glue/ruby_doc.c): create+init an
 * HTML parser, sanitize +html+ to UTF-8 bytes, run +parse+ (which calls the
 * appropriate lxb_html_parse_fragment* and returns the fragment root), free the
 * decoded input, and destroy the parser - returning the root, which is owned by
 * its (possibly transient) document and survives the parser's destruction.
 * Raises on parser-create / OOM / parse failure. Both fragment paths
 * (Document#fragment via a tag-id context, inner_html=/outer_html= via an element
 * context) share this so the create/sanitize/free/destroy + error cleanup lives
 * once. */
typedef lxb_dom_node_t *(*mkr_fragment_parse_fn)(lxb_html_parser_t *parser,
                                                 const lxb_char_t *hsrc, size_t hlen,
                                                 void *ctx);
lxb_dom_node_t *mkr_run_fragment_parser(VALUE html, mkr_fragment_parse_fn parse, void *ctx);

/* Node#clone_node(deep=false): shallow/deep DOM clone owned by this node's
 * document (import_node + <template>-content fixup), detached from any parent.
 * Implemented in ruby_doc.c (next to the import machinery), bound in
 * mkr_init_node. */
VALUE mkr_node_clone_node(int argc, VALUE *argv, VALUE self);

/* NodeSet bridge (glue/ruby_node_set.c). mkr_raw_node_t (above): callers cast
 * their typed node to it when pushing (forgetting the type is the safe, store
 * direction); the single typed read-back lives in mkr_node_set_wrap. */
VALUE mkr_node_set_new(VALUE document);
void  mkr_node_set_push(VALUE rb_set, mkr_raw_node_t *node);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_GLUE_H */
