/* ruby_xml_node_internal.h - shared between the two halves of the XML node glue.
 *
 * ruby_xml_node.c was 2,002 lines holding three separable concerns: reading the
 * tree, turning it back into text, and changing it. They are split so each can
 * be replaced by its Rust port on its own - a regression is only bisectable if
 * each part can be swapped alone.
 *
 * The boundary is one-directional: the readers use nothing from the writers.
 * Exactly two functions cross the other way and so are no longer static; the
 * rest of the shared surface was already public in glue.h. */
#ifndef MAKIRI_RUBY_XML_NODE_INTERNAL_H
#define MAKIRI_RUBY_XML_NODE_INTERNAL_H

#include "glue.h"
#include "../xml/mkr_xml_node.h"

#ifdef __cplusplus
extern "C" {
#endif

/* The keepalive Document of an XML node (XML-strict: raises for an HTML node). */
VALUE mkr_xml_node_document(VALUE self);

/* Wrap a node reached from +self+, under +self+'s Document. */
VALUE mkr_xml_wrap_rel(VALUE self, mkr_xml_node_t *rel);

/* Each half's registrations; called by mkr_init_xml_node. */
void mkr_init_xml_node_read(void);
void mkr_init_xml_node_serialize(void);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_RUBY_XML_NODE_INTERNAL_H */
